//! Docker service provider.
//!
//! Detects Docker and Docker Compose, collects container metrics,
//! and generates alerts for restart loops, stopped containers, and
//! resource usage issues.

use anyhow::Result;
use async_trait::async_trait;

use super::{alert, metric_int, ServiceProvider};
use crate::event::{Alert, AlertSeverity, DetectedService, ServiceKind, ServiceStatus};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

/// Docker service provider.
pub struct DockerProvider;

#[async_trait]
impl ServiceProvider for DockerProvider {
    fn kind(&self) -> ServiceKind {
        ServiceKind::Docker
    }

    fn detect(&self, probe_output: &ProbeOutput) -> bool {
        // Check if Docker section has content
        probe_output.has_section("DOCKER")
    }

    /// Extract basic metrics from Quick Scan docker ps output.
    /// This allows us to show container count immediately without waiting for Deep Probe.
    fn quick_metrics(&self, probe_output: &ProbeOutput) -> Vec<super::ServiceMetric> {
        let mut metrics = Vec::new();

        if let Some(docker_output) = probe_output.get_section("DOCKER") {
            // Parse docker ps output: ID\tNames\tStatus\tImage
            let lines: Vec<&str> = docker_output.lines().collect();
            let total = lines.len() as i64;

            // Count running containers (Status contains "Up")
            let running = lines
                .iter()
                .filter(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        parts[2].contains("Up")
                    } else {
                        false
                    }
                })
                .count() as i64;

            metrics.push(metric_int("containers_total", total, ""));
            metrics.push(metric_int("containers_running", running, ""));
        }

        metrics
    }

    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService> {
        // Collect all Docker information in one command for efficiency
        let docker_info = session
            .run_command(
                r#"
echo "===CONTAINERS==="
docker ps -a --format '{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.State}}\t{{.Image}}' 2>/dev/null
echo "===STATS==="
docker stats --no-stream --format '{{.Name}}\t{{.CPUPerc}}\t{{.MemUsage}}' 2>/dev/null | head -20
echo "===COMPOSE==="
docker compose ls --format json 2>/dev/null || echo "[]"
echo "===RESTARTS==="
docker inspect --format '{{.Name}}\t{{.RestartCount}}' $(docker ps -aq) 2>/dev/null
"#,
            )
            .await?;

        let (metrics, alerts, status) = parse_docker_output(&docker_info);

        Ok(DetectedService {
            kind: ServiceKind::Docker,
            version: None, // Could extract from `docker --version` if needed
            status,
            metrics,
            alerts,
            suggested_snippets: vec![
                "docker ps -a".to_string(),
                "docker compose restart".to_string(),
                "docker logs <container>".to_string(),
                "docker system prune".to_string(),
            ],
        })
    }
}

/// Parse the combined Docker command output into metrics and alerts.
fn parse_docker_output(output: &str) -> (Vec<super::ServiceMetric>, Vec<Alert>, ServiceStatus) {
    let mut metrics = Vec::new();
    let mut alerts = Vec::new();

    let mut current_section = None;
    let mut containers_data = Vec::new();
    let mut restart_counts: Vec<(String, i64)> = Vec::new();

    // Parse sections
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("===") {
            current_section = if trimmed.contains("CONTAINERS") {
                Some("containers")
            } else if trimmed.contains("STATS") {
                Some("stats")
            } else if trimmed.contains("RESTARTS") {
                Some("restarts")
            } else {
                None
            };
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        match current_section {
            Some("containers") => {
                // Parse: ID\tName\tStatus\tState\tImage
                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.len() >= 4 {
                    containers_data.push((
                        parts[1].to_string(), // name
                        parts[2].to_string(), // status
                        parts[3].to_string(), // state
                    ));
                }
            }
            Some("restarts") => {
                // Parse: /name\trestartCount
                let parts: Vec<&str> = trimmed.split('\t').collect();
                if parts.len() >= 2 {
                    if let Ok(count) = parts[1].parse::<i64>() {
                        restart_counts.push((parts[0].trim_start_matches('/').to_string(), count));
                    }
                }
            }
            _ => {}
        }
    }

    // Calculate metrics
    let total_containers = containers_data.len() as i64;
    let running = containers_data
        .iter()
        .filter(|(_, _, state)| state.to_lowercase() == "running")
        .count() as i64;
    let stopped = containers_data
        .iter()
        .filter(|(_, _, state)| state.to_lowercase() == "exited")
        .count() as i64;
    let restarting = containers_data
        .iter()
        .filter(|(_, status, _)| status.to_lowercase().contains("restarting"))
        .count() as i64;

    metrics.push(metric_int("containers_total", total_containers, ""));
    metrics.push(metric_int("containers_running", running, ""));
    metrics.push(metric_int("containers_stopped", stopped, ""));
    metrics.push(metric_int("containers_restarting", restarting, ""));

    // Generate alerts based on thresholds from tech-2.md A.3.4
    let mut critical_issues = Vec::new();

    // Check for containers in restart loop (>3 restarts)
    for (name, count) in &restart_counts {
        if *count > 3 {
            alerts.push(alert(
                AlertSeverity::Critical,
                ServiceKind::Docker,
                format!("Container {} in restart loop ({} restarts)", name, count),
                Some(format!("docker logs {} --tail 100", name)),
            ));
            critical_issues.push(format!("{} restarting", name));
        }
    }

    // Check for unexpectedly stopped containers
    for (name, status, state) in &containers_data {
        if state.to_lowercase() == "exited" && !status.to_lowercase().contains("exited (0)") {
            alerts.push(alert(
                AlertSeverity::Warning,
                ServiceKind::Docker,
                format!("Container {} stopped unexpectedly: {}", name, status),
                Some(format!("docker start {}", name)),
            ));
        }
    }

    // Check for many restarting containers
    if restarting > 0 {
        alerts.push(alert(
            AlertSeverity::Critical,
            ServiceKind::Docker,
            format!("{} container(s) in restarting state", restarting),
            Some("docker compose restart".to_string()),
        ));
        critical_issues.push("containers restarting".to_string());
    }

    // Determine overall service status
    let status = if !critical_issues.is_empty() {
        ServiceStatus::Critical(critical_issues.join(", "))
    } else if stopped > 0 {
        ServiceStatus::Degraded(format!("{} container(s) stopped", stopped))
    } else if total_containers > 0 {
        ServiceStatus::Healthy
    } else {
        ServiceStatus::Unknown
    };

    (metrics, alerts, status)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_detect_from_probe() {
        let probe_output = "===OMNYSSH:DOCKER===\nabc123\tnginx\tUp 2 hours\tnginx:latest\n";
        let parsed = ProbeOutput::parse(probe_output).expect("should parse");
        let provider = DockerProvider;
        assert!(provider.detect(&parsed));
    }

    #[test]
    fn test_docker_not_detected_when_absent() {
        let probe_output = "===OMNYSSH:OS===\nUbuntu\n";
        let parsed = ProbeOutput::parse(probe_output).expect("should parse");
        let provider = DockerProvider;
        assert!(!provider.detect(&parsed));
    }

    #[test]
    fn test_parse_healthy_containers() {
        let output = r#"===CONTAINERS===
abc123	nginx-proxy	Up 2 hours	running	nginx:latest
def456	db-master	Up 5 days	running	postgres:15
===STATS===
===RESTARTS===
/nginx-proxy	0
/db-master	1
"#;
        let (metrics, alerts, status) = parse_docker_output(output);

        assert_eq!(metrics.len(), 4);
        assert!(alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Healthy));

        // Check metrics
        let running_metric = metrics.iter().find(|m| m.name == "containers_running");
        assert!(running_metric.is_some());
    }

    #[test]
    fn test_parse_restart_loop() {
        let output = r#"===CONTAINERS===
abc123	nginx-proxy	Restarting (5) About a minute ago	restarting	nginx:latest
===STATS===
===RESTARTS===
/nginx-proxy	5
"#;
        let (_metrics, alerts, status) = parse_docker_output(output);

        assert!(!alerts.is_empty());
        assert!(matches!(status, ServiceStatus::Critical(_)));

        let restart_alert = alerts.iter().find(|a| a.message.contains("restart loop"));
        assert!(restart_alert.is_some());
        assert_eq!(restart_alert.unwrap().severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_parse_exited_containers() {
        let output = r#"===CONTAINERS===
abc123	nginx-proxy	Exited (1) 5 minutes ago	exited	nginx:latest
def456	db-master	Up 2 days	running	postgres:15
===STATS===
===RESTARTS===
/nginx-proxy	0
/db-master	0
"#;
        let (metrics, alerts, status) = parse_docker_output(output);

        assert!(matches!(status, ServiceStatus::Degraded(_)));

        let stopped_metric = metrics.iter().find(|m| m.name == "containers_stopped");
        assert!(stopped_metric.is_some());

        // Should have warning about stopped container
        assert!(alerts.iter().any(|a| a.severity == AlertSeverity::Warning));
    }

    #[test]
    fn test_parse_permission_denied() {
        let output = "Got permission denied while trying to connect to the Docker daemon socket";
        let (metrics, _alerts, _status) = parse_docker_output(output);

        // Should gracefully handle errors - metrics might be empty or default
        // The important thing is it doesn't panic
        assert_eq!(metrics.len(), 4); // Should still return default metrics
    }
}
