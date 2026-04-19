//! PostgreSQL service provider.
//!
//! Detects PostgreSQL, monitors replication lag, connection counts,
//! and waiting locks.

use anyhow::Result;
use async_trait::async_trait;

use super::{alert, metric_int, ServiceProvider};
use crate::event::{Alert, AlertSeverity, DetectedService, ServiceKind, ServiceStatus};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

/// PostgreSQL service provider.
pub struct PostgreSQLProvider;

#[async_trait]
impl ServiceProvider for PostgreSQLProvider {
    fn kind(&self) -> ServiceKind {
        ServiceKind::PostgreSQL
    }

    fn detect(&self, probe_output: &ProbeOutput) -> bool {
        // Check for postgresql service in systemd OR port 5432 in listening ports
        if let Some(services) = probe_output.get_section("SERVICES") {
            if services.contains("postgresql") {
                return true;
            }
        }
        if let Some(listen) = probe_output.get_section("LISTEN") {
            if listen.contains(":5432") || listen.contains("5432") {
                return true;
            }
        }
        false
    }

    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService> {
        // Collect PostgreSQL metrics (all in one batch)
        let pg_metrics = session
            .run_command(
                r#"
echo "===PG_STATUS==="
sudo -u postgres pg_isready 2>/dev/null || echo "not available"
echo "===PG_REPLICATION==="
sudo -u postgres psql -t -c "SELECT client_addr, state, sent_lsn, write_lsn, replay_lsn, COALESCE(extract(epoch from replay_lag), 0)::int as lag_seconds FROM pg_stat_replication;" 2>/dev/null
echo "===PG_CONNECTIONS==="
sudo -u postgres psql -t -c "SELECT count(*), state FROM pg_stat_activity GROUP BY state;" 2>/dev/null
echo "===PG_LOCKS==="
sudo -u postgres psql -t -c "SELECT count(*) FROM pg_locks WHERE NOT granted;" 2>/dev/null
"#,
            )
            .await?;

        let (metrics, alerts, status) = parse_pg_output(&pg_metrics);

        Ok(DetectedService {
            kind: ServiceKind::PostgreSQL,
            version: None,
            status,
            metrics,
            alerts,
            suggested_snippets: vec![
                "sudo -u postgres pg_isready".to_string(),
                "sudo -u postgres psql -c 'SELECT * FROM pg_stat_replication;'".to_string(),
                "sudo -u postgres psql -c 'SELECT count(*), state FROM pg_stat_activity GROUP BY state;'".to_string(),
            ],
        })
    }
}

fn parse_pg_output(output: &str) -> (Vec<super::ServiceMetric>, Vec<Alert>, ServiceStatus) {
    let mut metrics = Vec::new();
    let mut alerts = Vec::new();
    let mut max_repl_lag = 0i64;
    let mut waiting_locks = 0i64;
    let mut pg_ready = false;

    let mut current_section = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("===PG_") {
            current_section = if trimmed.contains("STATUS") {
                Some("status")
            } else if trimmed.contains("REPLICATION") {
                Some("replication")
            } else if trimmed.contains("LOCKS") {
                Some("locks")
            } else {
                None
            };
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        match current_section {
            Some("status") if trimmed.contains("accepting connections") => {
                pg_ready = true;
            }
            Some("replication") => {
                // Parse lag from last column
                let parts: Vec<&str> = trimmed.split('|').collect();
                if let Some(lag_str) = parts.last() {
                    if let Ok(lag) = lag_str.trim().parse::<i64>() {
                        max_repl_lag = max_repl_lag.max(lag);
                    }
                }
            }
            Some("locks") => {
                if let Ok(count) = trimmed.trim().parse::<i64>() {
                    waiting_locks = count;
                }
            }
            _ => {}
        }
    }

    // Add metrics
    metrics.push(metric_int("replication_lag_seconds", max_repl_lag, "s"));
    metrics.push(metric_int("waiting_locks", waiting_locks, ""));

    // Generate alerts per spec (tech-2.md A.3.4)
    let mut critical_issues = Vec::new();

    if !pg_ready {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::PostgreSQL,
            "PostgreSQL not accepting connections",
            Some("sudo systemctl status postgresql".to_string()),
        ));
        critical_issues.push("not ready");
    }

    if max_repl_lag > 10 {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::PostgreSQL,
            format!("Replication lag > 10s: {}s", max_repl_lag),
            Some("Check replication status".to_string()),
        ));
        critical_issues.push("replication lag");
    }

    if waiting_locks > 5 {
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::PostgreSQL,
            format!("{} waiting locks detected", waiting_locks),
            Some(
                "sudo -u postgres psql -c 'SELECT * FROM pg_locks WHERE NOT granted;'".to_string(),
            ),
        ));
    }

    let status = if !critical_issues.is_empty() {
        ServiceStatus::Critical(critical_issues.join(", "))
    } else if waiting_locks > 0 {
        ServiceStatus::Degraded(format!("{} waiting locks", waiting_locks))
    } else if pg_ready {
        ServiceStatus::Healthy
    } else {
        ServiceStatus::Unknown
    };

    (metrics, alerts, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pg_detect_from_systemd() {
        let probe = "===OMNYSSH:SERVICES===\npostgresql.service\nsshd.service\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(PostgreSQLProvider.detect(&parsed));
    }

    #[test]
    fn test_pg_detect_from_port() {
        let probe = "===OMNYSSH:LISTEN===\n0.0.0.0:5432\tLISTEN\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(PostgreSQLProvider.detect(&parsed));
    }

    #[test]
    fn test_parse_replication_healthy() {
        let output = "===PG_STATUS===\naccepting connections\n===PG_REPLICATION===\n192.168.1.10 | streaming | | | | 2\n===PG_LOCKS===\n0\n";
        let (_metrics, alerts, status) = parse_pg_output(output);
        assert!(alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Healthy));
    }

    #[test]
    fn test_parse_replication_lagging() {
        let output = "===PG_STATUS===\naccepting connections\n===PG_REPLICATION===\n192.168.1.10 | streaming | | | | 15\n===PG_LOCKS===\n0\n";
        let (metrics, alerts, status) = parse_pg_output(output);
        assert!(!alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Critical(_)));

        let lag_metric = metrics.iter().find(|m| m.name == "replication_lag_seconds");
        assert!(lag_metric.is_some());
    }
}
