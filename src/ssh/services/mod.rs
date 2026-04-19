//! Service provider registry and trait definitions.
//!
//! Each service (Docker, PostgreSQL, Nginx, etc.) implements the
//! [`ServiceProvider`] trait. The registry discovers which providers
//! apply to a given server based on the probe output.

use anyhow::Result;
use async_trait::async_trait;

use crate::event::{Alert, DetectedService, ServiceKind, ServiceMetric};
use crate::ssh::probe::ProbeOutput;
use crate::ssh::session::SshSession;

// Service provider modules
pub mod docker;
pub mod nginx;
pub mod nodejs;
pub mod postgresql;
pub mod redis;

/// Trait for service-specific detection and metric collection.
///
/// Each provider implements:
/// 1. Quick detection from probe output
/// 2. Deep metric collection commands
/// 3. Parsing of metric output into structured data
/// 4. Alert generation based on thresholds
/// 5. Suggested snippet templates
#[async_trait]
pub trait ServiceProvider: Send + Sync {
    /// Returns the service type this provider handles.
    fn kind(&self) -> ServiceKind;

    /// Quick check: is this service present on the server?
    ///
    /// Called during Quick Scan with the parsed probe output.
    /// Should be fast — only check for presence, not detailed metrics.
    fn detect(&self, probe_output: &ProbeOutput) -> bool;

    /// Extract basic metrics from Quick Scan probe output.
    ///
    /// This is called immediately during Quick Scan to provide basic
    /// service information without waiting for Deep Probe.
    /// Default implementation returns empty metrics.
    fn quick_metrics(&self, _probe_output: &ProbeOutput) -> Vec<ServiceMetric> {
        Vec::new()
    }

    /// Collect detailed metrics for this service.
    ///
    /// Called during Deep Probe. Returns commands to execute and
    /// parses their output.
    ///
    /// # Errors
    /// Returns an error if SSH commands fail or output cannot be parsed.
    /// Providers should handle missing commands gracefully.
    async fn collect_metrics(&self, session: &SshSession) -> Result<DetectedService>;

    /// Extract service version from probe output, if available.
    ///
    /// Returns None if version cannot be determined (this is fine).
    fn extract_version(&self, _probe_output: &ProbeOutput) -> Option<String> {
        None
    }
}

/// Service registry that manages all available providers.
pub struct ServiceRegistry {
    providers: Vec<Box<dyn ServiceProvider>>,
}

impl ServiceRegistry {
    /// Create a new registry with all built-in providers.
    /// Only 5 core services are supported: Docker, Nginx, PostgreSQL, Redis, Node.js.
    pub fn new() -> Self {
        let providers: Vec<Box<dyn ServiceProvider>> = vec![
            Box::new(docker::DockerProvider),
            Box::new(nginx::NginxProvider),
            Box::new(postgresql::PostgreSQLProvider),
            Box::new(redis::RedisProvider),
            Box::new(nodejs::NodeJSProvider),
        ];

        Self { providers }
    }

    /// Detect which services are present based on probe output.
    ///
    /// Returns a list of service kinds that were detected.
    pub fn detect_services(&self, probe_output: &ProbeOutput) -> Vec<ServiceKind> {
        self.providers
            .iter()
            .filter(|p| p.detect(probe_output))
            .map(|p| p.kind())
            .collect()
    }

    /// Get a provider by service kind.
    pub fn get_provider(&self, kind: &ServiceKind) -> Option<&dyn ServiceProvider> {
        self.providers
            .iter()
            .find(|p| &p.kind() == kind)
            .map(|boxed| &**boxed)
    }

    /// Collect metrics for all detected services.
    ///
    /// Runs metric collection in parallel for all providers.
    ///
    /// # Errors
    /// Returns errors from individual providers, but doesn't fail the entire
    /// collection if one provider fails (graceful degradation).
    pub async fn collect_all_metrics(
        &self,
        session: &SshSession,
        probe_output: &ProbeOutput,
    ) -> Vec<DetectedService> {
        let detected_kinds = self.detect_services(probe_output);

        // Collect metrics for each detected service
        let mut services = Vec::new();
        for kind in detected_kinds {
            if let Some(provider) = self.get_provider(&kind) {
                match provider.collect_metrics(session).await {
                    Ok(service) => services.push(service),
                    Err(e) => {
                        tracing::debug!(
                            service = ?kind,
                            error = %e,
                            "failed to collect metrics for service"
                        );
                    }
                }
            }
        }

        services
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create a simple service metric.
pub fn metric_int(name: impl Into<String>, value: i64, unit: impl Into<String>) -> ServiceMetric {
    ServiceMetric {
        name: name.into(),
        value: crate::event::MetricValue::Integer(value),
        unit: unit.into(),
        threshold: None,
    }
}

/// Helper to create a float metric.
pub fn metric_float(name: impl Into<String>, value: f64, unit: impl Into<String>) -> ServiceMetric {
    ServiceMetric {
        name: name.into(),
        value: crate::event::MetricValue::Float(value),
        unit: unit.into(),
        threshold: None,
    }
}

/// Helper to create a string metric.
pub fn metric_string(name: impl Into<String>, value: impl Into<String>) -> ServiceMetric {
    ServiceMetric {
        name: name.into(),
        value: crate::event::MetricValue::String(value.into()),
        unit: String::new(),
        threshold: None,
    }
}

/// Helper to create an alert.
pub fn alert(
    severity: crate::event::AlertSeverity,
    service: ServiceKind,
    message: impl Into<String>,
    suggested_action: Option<String>,
) -> Alert {
    Alert {
        severity,
        message: message.into(),
        service,
        suggested_action,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_creation() {
        let registry = ServiceRegistry::new();
        assert_eq!(registry.providers.len(), 5); // Docker, Nginx, PostgreSQL, Redis, Node.js
    }

    #[test]
    fn test_registry_get_provider() {
        let registry = ServiceRegistry::new();
        assert!(registry.get_provider(&ServiceKind::Docker).is_some());
        assert!(registry.get_provider(&ServiceKind::Nginx).is_some());
        assert!(registry.get_provider(&ServiceKind::PostgreSQL).is_some());
        assert!(registry.get_provider(&ServiceKind::Redis).is_some());
        assert!(registry.get_provider(&ServiceKind::NodeJS).is_some());
    }

    #[test]
    fn test_metric_helpers() {
        let m_int = metric_int("count", 42, "");
        assert_eq!(m_int.name, "count");
        matches!(m_int.value, crate::event::MetricValue::Integer(42));

        let m_float = metric_float("percent", 73.5, "%");
        assert_eq!(m_float.unit, "%");

        let m_string = metric_string("status", "healthy");
        matches!(m_string.value, crate::event::MetricValue::String(_));
    }
}
