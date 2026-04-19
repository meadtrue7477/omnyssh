//! Service discovery orchestrator.
//!
//! Coordinates Quick Scan and Deep Probe operations to detect and monitor
//! services on remote servers.
//!
//! Architecture:
//! - Quick Scan runs once per connection, discovers which services exist
//! - Deep Probe runs periodically to collect detailed metrics
//! - All operations are async and use the existing SSH session
//! - Results are sent via AppEvent to the main loop

use anyhow::Result;
use tokio::sync::mpsc;

use crate::event::{AppEvent, DetectedService};
use crate::ssh::probe::{generate_quick_scan_script, ProbeOutput};
use crate::ssh::services::ServiceRegistry;
use crate::ssh::session::SshSession;

/// Performs a Quick Scan on the given SSH session.
///
/// Quick Scan:
/// 1. Executes the probe script (single SSH command)
/// 2. Parses the output into sections
/// 3. Detects which services are present using the service registry
/// 4. Sends DiscoveryQuickScanDone event with minimal service info
///
/// This is fast (~2-3 seconds) and designed to run on first connection.
///
/// # Errors
/// Returns an error if the SSH command fails or times out.
/// Parsing errors are handled gracefully.
pub async fn quick_scan(
    session: &SshSession,
    host_id: String,
    tx: mpsc::Sender<AppEvent>,
) -> Result<()> {
    tracing::debug!(host = %host_id, "starting quick scan");

    // Execute probe script
    let probe_script = generate_quick_scan_script();
    let output = session.run_command(probe_script).await?;

    // Parse output
    let probe_output = ProbeOutput::parse(&output)?;

    // Detect services using registry
    let registry = ServiceRegistry::new();
    let detected_kinds = registry.detect_services(&probe_output);

    tracing::debug!(
        host = %host_id,
        services = ?detected_kinds,
        "quick scan detected {} services",
        detected_kinds.len()
    );

    // Create service info with basic quick metrics
    let mut services = Vec::new();
    for kind in detected_kinds {
        // Try to extract quick metrics from probe output
        let quick_metrics = if let Some(provider) = registry.get_provider(&kind) {
            provider.quick_metrics(&probe_output)
        } else {
            Vec::new()
        };

        services.push(DetectedService {
            kind: kind.clone(),
            version: None,
            status: crate::event::ServiceStatus::Unknown,
            metrics: quick_metrics,
            alerts: Vec::new(),
            suggested_snippets: Vec::new(),
        });
    }

    // Extract OS information and send via MetricsUpdate
    if let Some(os_info) = probe_output.parse_os_info() {
        use crate::event::Metrics;
        use std::time::Instant;

        // Send a partial metrics update with just OS info
        let metrics = Metrics {
            cpu_percent: None,
            ram_percent: None,
            disk_percent: None,
            uptime: None,
            load_avg: None,
            os_info: Some(os_info),
            last_updated: Instant::now(),
        };

        tx.send(AppEvent::MetricsUpdate(host_id.clone(), metrics))
            .await
            .ok(); // Don't fail discovery if we can't send OS info
    }

    // Send event to main loop
    tx.send(AppEvent::DiscoveryQuickScanDone(host_id, services))
        .await
        .map_err(|_| anyhow::anyhow!("event channel closed"))?;

    Ok(())
}

/// Performs a Deep Probe on the given SSH session.
///
/// Deep Probe:
/// 1. For each detected service, runs detailed metric collection
/// 2. Parses metrics and generates alerts based on thresholds
/// 3. Sends DiscoveryDeepProbeDone event with full service details
///
/// This is slower (~10-30 seconds depending on number of services) and
/// runs periodically based on `deep_probe_interval` config.
///
/// # Errors
/// Returns an error if critical SSH commands fail.
/// Individual service failures are logged but don't fail the entire probe
/// (graceful degradation).
pub async fn deep_probe(
    session: &SshSession,
    host_id: String,
    probe_output: &ProbeOutput,
    tx: mpsc::Sender<AppEvent>,
) -> Result<()> {
    tracing::debug!(host = %host_id, "starting deep probe");

    // Collect metrics for all detected services
    let registry = ServiceRegistry::new();
    let services = registry.collect_all_metrics(session, probe_output).await;

    tracing::debug!(
        host = %host_id,
        services = services.len(),
        "deep probe collected metrics for {} services",
        services.len()
    );

    // Extract and send alerts
    for service in &services {
        for alert in &service.alerts {
            tx.send(AppEvent::AlertNew(host_id.clone(), alert.clone()))
                .await
                .ok(); // Don't fail if alert channel is full
        }
    }

    // Send aggregated service data
    tx.send(AppEvent::DiscoveryDeepProbeDone(host_id, services))
        .await
        .map_err(|_| anyhow::anyhow!("event channel closed"))?;

    Ok(())
}

// Note: Quick Scan and Deep Probe are called directly from pool.rs
// They run in spawned tokio tasks there to avoid blocking the metrics loop

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_scan_script_generation() {
        let script = generate_quick_scan_script();
        assert!(script.contains("===OMNYSSH:OS==="));
        assert!(script.contains("===OMNYSSH:DOCKER==="));
        assert!(script.contains("===OMNYSSH:SERVICES==="));
    }

    // Integration tests with mock SSH would go here
    // For now, we rely on the probe and service provider unit tests
}
