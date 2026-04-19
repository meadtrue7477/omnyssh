//! Node.js service provider.
//!
//! Detects Node.js processes and PM2-managed applications,
//! monitors resource usage and process health.

use anyhow::Result;
use async_trait::async_trait;

use super::{alert, metric_int, ServiceProvider};
use crate::event::{Alert, AlertSeverity, DetectedService, ServiceKind, ServiceStatus};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

/// Node.js service provider.
pub struct NodeJSProvider;

#[async_trait]
impl ServiceProvider for NodeJSProvider {
    fn kind(&self) -> ServiceKind {
        ServiceKind::NodeJS
    }

    fn detect(&self, probe_output: &ProbeOutput) -> bool {
        // Check for node processes in PROCESS section
        if let Some(processes) = probe_output.get_section("PROCESS") {
            if processes.contains("node ") || processes.contains("/node") {
                return true;
            }
        }
        false
    }

    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService> {
        // Collect Node.js process info and PM2 status in one command
        let nodejs_info = session
            .run_command(
                r#"
echo "===NODE_PROCESSES==="
ps aux 2>/dev/null | grep -E '[n]ode ' | head -20
echo "===PM2_STATUS==="
pm2 jlist 2>/dev/null || echo "[]"
"#,
            )
            .await?;

        let (metrics, alerts, status) = parse_nodejs_output(&nodejs_info);

        Ok(DetectedService {
            kind: ServiceKind::NodeJS,
            version: None,
            status,
            metrics,
            alerts,
            suggested_snippets: vec![
                "pm2 status".to_string(),
                "pm2 logs".to_string(),
                "pm2 monit".to_string(),
                "ps aux | grep node".to_string(),
            ],
        })
    }
}

/// Parse the combined Node.js/PM2 output into metrics and alerts.
fn parse_nodejs_output(output: &str) -> (Vec<super::ServiceMetric>, Vec<Alert>, ServiceStatus) {
    let mut metrics = Vec::new();
    let mut alerts = Vec::new();

    let mut current_section = None;
    let mut node_processes: Vec<NodeProcess> = Vec::new();
    let mut pm2_apps_errored = 0;
    let mut pm2_apps_online = 0;
    let mut pm2_high_restarts = Vec::new();

    // Parse sections
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("===NODE_") {
            current_section = if trimmed.contains("PROCESSES") {
                Some("processes")
            } else {
                None
            };
            continue;
        } else if trimmed.starts_with("===PM2_") {
            current_section = if trimmed.contains("STATUS") {
                Some("pm2")
            } else {
                None
            };
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        match current_section {
            Some("processes") => {
                // Parse ps aux output: user pid %cpu %mem ...
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 4 {
                    if let (Ok(cpu), Ok(mem)) = (parts[2].parse::<f64>(), parts[3].parse::<f64>()) {
                        node_processes.push(NodeProcess {
                            cpu_percent: cpu,
                            mem_percent: mem,
                        });
                    }
                }
            }
            Some("pm2") => {
                // PM2 JSON list - simple parsing for status and restarts
                if trimmed.contains("\"status\":") {
                    if trimmed.contains("\"status\":\"errored\"")
                        || trimmed.contains("\"status\":\"stopped\"")
                    {
                        pm2_apps_errored += 1;
                    } else if trimmed.contains("\"status\":\"online\"") {
                        pm2_apps_online += 1;
                    }
                }
                // Check for high restart count (simple heuristic: look for restart: >10)
                if let Some(restart_pos) = trimmed.find("\"restart\":") {
                    let after = &trimmed[restart_pos + 10..];
                    // Find the end of the value - either comma or closing brace
                    let end_pos = after
                        .find(',')
                        .or_else(|| after.find('}'))
                        .unwrap_or(after.len());
                    if let Ok(restarts) = after[..end_pos].trim().parse::<i64>() {
                        if restarts > 10 {
                            pm2_high_restarts.push(restarts);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Calculate metrics
    let total_node_processes = node_processes.len() as i64;
    let high_cpu_count = node_processes
        .iter()
        .filter(|p| p.cpu_percent > 90.0)
        .count() as i64;
    let high_mem_count = node_processes
        .iter()
        .filter(|p| p.mem_percent > 50.0)
        .count() as i64;

    metrics.push(metric_int("node_processes", total_node_processes, ""));
    metrics.push(metric_int("pm2_apps_online", pm2_apps_online, ""));
    metrics.push(metric_int("pm2_apps_errored", pm2_apps_errored, ""));
    if !pm2_high_restarts.is_empty() {
        let max_restarts = *pm2_high_restarts.iter().max().unwrap_or(&0);
        metrics.push(metric_int("max_pm2_restarts", max_restarts, ""));
    }

    // Generate alerts (simple, essential checks)
    let mut critical_issues = Vec::new();

    // Critical: PM2 apps in errored state
    if pm2_apps_errored > 0 {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::NodeJS,
            format!("{} PM2 app(s) in errored state", pm2_apps_errored),
            Some("Check PM2 logs: pm2 logs".to_string()),
        ));
        critical_issues.push("PM2 apps errored");
    }

    // Critical: Node process with very high CPU
    if high_cpu_count > 0 {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::NodeJS,
            format!("{} Node process(es) with >90% CPU", high_cpu_count),
            Some("Check process: ps aux | grep node".to_string()),
        ));
        critical_issues.push("high CPU");
    }

    // Warning: High memory usage
    if high_mem_count > 0 {
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::NodeJS,
            format!("{} Node process(es) with >50% memory", high_mem_count),
            None,
        ));
    }

    // Warning: PM2 apps restarting frequently
    if !pm2_high_restarts.is_empty() {
        let max_restarts = *pm2_high_restarts.iter().max().unwrap_or(&0);
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::NodeJS,
            format!("PM2 app restarted {} times (high)", max_restarts),
            Some("Review PM2 logs for errors".to_string()),
        ));
    }

    // Determine overall service status
    let status = if !critical_issues.is_empty() {
        ServiceStatus::Critical(critical_issues.join(", "))
    } else if !pm2_high_restarts.is_empty() || high_mem_count > 0 {
        ServiceStatus::Degraded("performance issues".to_string())
    } else if total_node_processes > 0 || pm2_apps_online > 0 {
        ServiceStatus::Healthy
    } else {
        ServiceStatus::Unknown
    };

    (metrics, alerts, status)
}

/// Represents a Node.js process from ps aux.
struct NodeProcess {
    cpu_percent: f64,
    mem_percent: f64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nodejs_detect_from_process() {
        let probe = "===OMNYSSH:PROCESS===\nuser 1234 /usr/bin/node server.js\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(NodeJSProvider.detect(&parsed));
    }

    #[test]
    fn test_nodejs_not_detected() {
        let probe = "===OMNYSSH:SERVICES===\nsshd.service\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(!NodeJSProvider.detect(&parsed));
    }

    #[test]
    fn test_parse_healthy_nodejs() {
        let output = r#"===NODE_PROCESSES===
user     1234  2.5  1.2 1234567 123456 ?  Ssl  10:00   0:05 /usr/bin/node server.js
===PM2_STATUS===
[{"name":"app1","status":"online","restart":2}]
"#;
        let (metrics, alerts, status) = parse_nodejs_output(output);

        assert!(alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Healthy));

        let proc_metric = metrics.iter().find(|m| m.name == "node_processes");
        assert!(proc_metric.is_some());
    }

    #[test]
    fn test_parse_nodejs_high_cpu() {
        let output = r#"===NODE_PROCESSES===
user     1234  95.5  1.2 1234567 123456 ?  Rsl  10:00   0:05 /usr/bin/node server.js
===PM2_STATUS===
[]
"#;
        let (_metrics, alerts, status) = parse_nodejs_output(output);

        assert!(!alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Critical(_)));

        let cpu_alert = alerts.iter().find(|a| a.message.contains("CPU"));
        assert!(cpu_alert.is_some());
    }

    #[test]
    fn test_parse_pm2_errored() {
        let output = r#"===NODE_PROCESSES===
===PM2_STATUS===
[{"name":"app1","status":"errored","restart":5}]
"#;
        let (_metrics, alerts, status) = parse_nodejs_output(output);

        assert!(matches!(status, ServiceStatus::Critical(_)));

        let errored_alert = alerts.iter().find(|a| a.message.contains("errored"));
        assert!(errored_alert.is_some());
    }

    #[test]
    fn test_parse_pm2_high_restarts() {
        let output = r#"===NODE_PROCESSES===
===PM2_STATUS===
[{"name":"app1","status":"online","restart":25}]
"#;
        let (_metrics, alerts, status) = parse_nodejs_output(output);

        assert!(matches!(status, ServiceStatus::Degraded(_)));

        let restart_alert = alerts.iter().find(|a| a.message.contains("restarted"));
        assert!(restart_alert.is_some());
    }
}
