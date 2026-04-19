//! Redis service provider.
//!
//! Detects Redis servers, monitors memory usage, connection counts,
//! and keyspace statistics.

use anyhow::Result;
use async_trait::async_trait;

use super::{alert, metric_int, ServiceProvider};
use crate::event::{Alert, AlertSeverity, DetectedService, ServiceKind, ServiceStatus};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

/// Redis service provider.
pub struct RedisProvider;

#[async_trait]
impl ServiceProvider for RedisProvider {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Redis
    }

    fn detect(&self, probe_output: &ProbeOutput) -> bool {
        // Check for Redis on port 6379 in listening ports OR redis process
        if let Some(listen) = probe_output.get_section("LISTEN") {
            if listen.contains(":6379") || listen.contains("6379") {
                return true;
            }
        }
        if let Some(processes) = probe_output.get_section("PROCESS") {
            if processes.contains("redis-server") || processes.contains("redis") {
                return true;
            }
        }
        false
    }

    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService> {
        // Collect Redis INFO in one command for efficiency
        let redis_info = session
            .run_command(
                r#"
echo "===REDIS_PING==="
redis-cli ping 2>/dev/null || echo "ERROR"
echo "===REDIS_INFO==="
redis-cli info memory 2>/dev/null | grep -E 'used_memory:|used_memory_peak:|maxmemory:|evicted_keys:' 2>/dev/null
echo "===REDIS_CLIENTS==="
redis-cli info clients 2>/dev/null | grep -E 'connected_clients:|blocked_clients:|maxclients:' 2>/dev/null
echo "===REDIS_STATS==="
redis-cli info stats 2>/dev/null | grep -E 'total_connections_received:|keyspace_hits:|keyspace_misses:' 2>/dev/null
"#,
            )
            .await?;

        let (metrics, alerts, status) = parse_redis_output(&redis_info);

        Ok(DetectedService {
            kind: ServiceKind::Redis,
            version: None,
            status,
            metrics,
            alerts,
            suggested_snippets: vec![
                "redis-cli info".to_string(),
                "redis-cli ping".to_string(),
                "redis-cli client list".to_string(),
                "redis-cli info memory".to_string(),
            ],
        })
    }
}

/// Parse the combined Redis INFO output into metrics and alerts.
fn parse_redis_output(output: &str) -> (Vec<super::ServiceMetric>, Vec<Alert>, ServiceStatus) {
    let mut metrics = Vec::new();
    let mut alerts = Vec::new();
    let mut redis_responding = false;

    let mut current_section = None;
    let mut memory_used = 0i64;
    let mut memory_max = 0i64;
    let mut connected_clients = 0i64;
    let mut evicted_keys = 0i64;

    // Parse sections
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("===REDIS_") {
            current_section = if trimmed.contains("PING") {
                Some("ping")
            } else if trimmed.contains("INFO") {
                Some("info")
            } else if trimmed.contains("CLIENTS") {
                Some("clients")
            } else if trimmed.contains("STATS") {
                Some("stats")
            } else {
                None
            };
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        match current_section {
            Some("ping") if trimmed.contains("PONG") => {
                redis_responding = true;
            }
            Some("info") => {
                if let Some(val) = parse_redis_kv(trimmed, "used_memory:") {
                    memory_used = (val / (1024 * 1024)) as i64; // Convert to MB
                } else if let Some(val) = parse_redis_kv(trimmed, "maxmemory:") {
                    memory_max = (val / (1024 * 1024)) as i64; // Convert to MB
                } else if let Some(val) = parse_redis_kv(trimmed, "evicted_keys:") {
                    evicted_keys = val as i64;
                }
            }
            Some("clients") => {
                if let Some(val) = parse_redis_kv(trimmed, "connected_clients:") {
                    connected_clients = val as i64;
                }
            }
            _ => {}
        }
    }

    // Add metrics
    metrics.push(metric_int("memory_used_mb", memory_used, "MB"));
    if memory_max > 0 {
        metrics.push(metric_int("memory_max_mb", memory_max, "MB"));
    }
    metrics.push(metric_int("connected_clients", connected_clients, ""));
    metrics.push(metric_int("evicted_keys", evicted_keys, ""));

    // Generate alerts (simple, essential checks)
    let mut critical_issues = Vec::new();

    // Critical: Redis not responding
    if !redis_responding {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::Redis,
            "Redis not responding to PING",
            Some("Check Redis service: sudo systemctl status redis".to_string()),
        ));
        critical_issues.push("not responding");
    }

    // Critical: Memory usage > 90% of maxmemory
    if memory_max > 0 {
        let mem_percent = (memory_used as f64 / memory_max as f64) * 100.0;
        if mem_percent > 90.0 {
            alerts.push(alert(
                AlertSeverity::Critical,
                ServiceKind::Redis,
                format!(
                    "Memory usage critical: {:.1}% ({}/{}MB)",
                    mem_percent, memory_used, memory_max
                ),
                Some("Consider increasing maxmemory or reviewing eviction policy".to_string()),
            ));
            critical_issues.push("memory critical");
        } else if mem_percent > 80.0 {
            alerts.push(alert(
                AlertSeverity::Warning,
                ServiceKind::Redis,
                format!(
                    "Memory usage high: {:.1}% ({}/{}MB)",
                    mem_percent, memory_used, memory_max
                ),
                None,
            ));
        }
    }

    // Warning: Evicted keys detected
    if evicted_keys > 100 {
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::Redis,
            format!("{} keys evicted (memory pressure)", evicted_keys),
            Some("Review maxmemory-policy setting".to_string()),
        ));
    }

    // Warning: Too many connected clients (simple heuristic: >1000)
    if connected_clients > 1000 {
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::Redis,
            format!("{} connected clients (high)", connected_clients),
            Some("Check for connection leaks".to_string()),
        ));
    }

    // Determine overall service status
    let status = if !critical_issues.is_empty() {
        ServiceStatus::Critical(critical_issues.join(", "))
    } else if evicted_keys > 100 || (memory_max > 0 && memory_used as f64 / memory_max as f64 > 0.8)
    {
        ServiceStatus::Degraded("memory pressure".to_string())
    } else if redis_responding {
        ServiceStatus::Healthy
    } else {
        ServiceStatus::Unknown
    };

    (metrics, alerts, status)
}

/// Helper to parse Redis INFO key:value format.
fn parse_redis_kv(line: &str, key: &str) -> Option<u64> {
    if line.starts_with(key) {
        line.strip_prefix(key)?.trim().parse::<u64>().ok()
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redis_detect_from_port() {
        let probe = "===OMNYSSH:LISTEN===\n0.0.0.0:6379\tLISTEN\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(RedisProvider.detect(&parsed));
    }

    #[test]
    fn test_redis_detect_from_process() {
        let probe = "===OMNYSSH:PROCESS===\nredis 1234 redis-server\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(RedisProvider.detect(&parsed));
    }

    #[test]
    fn test_redis_not_detected() {
        let probe = "===OMNYSSH:SERVICES===\nsshd.service\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(!RedisProvider.detect(&parsed));
    }

    #[test]
    fn test_parse_healthy_redis() {
        let output = r#"===REDIS_PING===
PONG
===REDIS_INFO===
used_memory:52428800
maxmemory:1073741824
evicted_keys:0
===REDIS_CLIENTS===
connected_clients:5
"#;
        let (metrics, alerts, status) = parse_redis_output(output);

        assert!(alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Healthy));

        let mem_metric = metrics.iter().find(|m| m.name == "memory_used_mb");
        assert!(mem_metric.is_some());
    }

    #[test]
    fn test_parse_redis_memory_critical() {
        let output = r#"===REDIS_PING===
PONG
===REDIS_INFO===
used_memory:1020054733
maxmemory:1073741824
evicted_keys:0
===REDIS_CLIENTS===
connected_clients:5
"#;
        let (_metrics, alerts, status) = parse_redis_output(output);

        assert!(!alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Critical(_)));

        let critical_alert = alerts
            .iter()
            .find(|a| a.severity == AlertSeverity::Critical);
        assert!(critical_alert.is_some());
        assert!(critical_alert
            .unwrap()
            .message
            .contains("Memory usage critical"));
    }

    #[test]
    fn test_parse_redis_not_responding() {
        let output = r#"===REDIS_PING===
ERROR
===REDIS_INFO===
===REDIS_CLIENTS===
"#;
        let (_metrics, alerts, status) = parse_redis_output(output);

        assert!(matches!(status, ServiceStatus::Critical(_)));
        let ping_alert = alerts.iter().find(|a| a.message.contains("not responding"));
        assert!(ping_alert.is_some());
    }

    #[test]
    fn test_parse_redis_evictions() {
        let output = r#"===REDIS_PING===
PONG
===REDIS_INFO===
used_memory:52428800
maxmemory:1073741824
evicted_keys:500
===REDIS_CLIENTS===
connected_clients:5
"#;
        let (_metrics, alerts, status) = parse_redis_output(output);

        assert!(matches!(status, ServiceStatus::Degraded(_)));
        let evict_alert = alerts.iter().find(|a| a.message.contains("evicted"));
        assert!(evict_alert.is_some());
    }
}
