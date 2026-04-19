//! Nginx service provider.
//!
//! Detects Nginx web server, monitors error logs for 502/504 errors,
//! and validates configuration.

use anyhow::Result;
use async_trait::async_trait;

use super::{alert, metric_int, ServiceProvider};
use crate::event::{Alert, AlertSeverity, DetectedService, ServiceKind, ServiceStatus};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

/// Nginx service provider.
pub struct NginxProvider;

#[async_trait]
impl ServiceProvider for NginxProvider {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Nginx
    }

    fn detect(&self, probe_output: &ProbeOutput) -> bool {
        // Check for nginx in systemd services OR nginx process
        if let Some(services) = probe_output.get_section("SERVICES") {
            if services.contains("nginx") {
                return true;
            }
        }
        if let Some(processes) = probe_output.get_section("PROCESS") {
            if processes.contains("nginx") {
                return true;
            }
        }
        false
    }

    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService> {
        let nginx_info = session
            .run_command(
                r#"
echo "===NGINX_CONFIG==="
nginx -t 2>&1
echo "===NGINX_ERRORS==="
tail -20 /var/log/nginx/error.log 2>/dev/null | grep -E '502|504' 2>/dev/null | wc -l
"#,
            )
            .await?;

        let (metrics, alerts, status) = parse_nginx_output(&nginx_info);

        Ok(DetectedService {
            kind: ServiceKind::Nginx,
            version: None,
            status,
            metrics,
            alerts,
            suggested_snippets: vec![
                "sudo nginx -t".to_string(),
                "sudo systemctl reload nginx".to_string(),
                "sudo tail -f /var/log/nginx/error.log".to_string(),
            ],
        })
    }
}

fn parse_nginx_output(output: &str) -> (Vec<super::ServiceMetric>, Vec<Alert>, ServiceStatus) {
    let mut metrics = Vec::new();
    let mut alerts = Vec::new();
    let mut config_ok = true;
    let mut error_count = 0i64;

    let mut current_section = None;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("===NGINX_") {
            current_section = if trimmed.contains("CONFIG") {
                Some("config")
            } else if trimmed.contains("ERRORS") {
                Some("errors")
            } else {
                None
            };
            continue;
        }

        match current_section {
            Some("config") if (trimmed.contains("test failed") || trimmed.contains("error")) => {
                config_ok = false;
            }
            Some("errors") => {
                if let Ok(count) = trimmed.trim().parse::<i64>() {
                    error_count = count;
                }
            }
            _ => {}
        }
    }

    metrics.push(metric_int("recent_502_504_errors", error_count, ""));

    // Generate alerts (tech-2.md A.3.4)
    if !config_ok {
        alerts.push(alert(
            AlertSeverity::Warning,
            ServiceKind::Nginx,
            "Nginx configuration has errors",
            Some("sudo nginx -t".to_string()),
        ));
    }

    if error_count > 0 {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::Nginx,
            format!("{} 502/504 errors in recent logs", error_count),
            Some("Check upstream services".to_string()),
        ));
    }

    let status = if !config_ok || error_count > 0 {
        ServiceStatus::Degraded("errors detected".to_string())
    } else {
        ServiceStatus::Healthy
    };

    (metrics, alerts, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nginx_detect_from_process() {
        let probe = "===OMNYSSH:PROCESS===\nroot 1234 nginx: master process\n";
        let parsed = ProbeOutput::parse(probe).expect("should parse");
        assert!(NginxProvider.detect(&parsed));
    }

    #[test]
    fn test_parse_nginx_healthy() {
        let output = "===NGINX_CONFIG===\nnginx: configuration file /etc/nginx/nginx.conf test is successful\n===NGINX_ERRORS===\n0\n";
        let (_metrics, alerts, status) = parse_nginx_output(output);
        assert!(alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Healthy));
    }

    #[test]
    fn test_parse_nginx_config_error() {
        let output =
            "===NGINX_CONFIG===\nnginx: configuration file test failed\n===NGINX_ERRORS===\n0\n";
        let (_metrics, alerts, status) = parse_nginx_output(output);
        assert!(!alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Degraded(_)));
    }
}
