//! SSH connection management.
//!
//! Connections delegated to the system SSH binary.
//! Also provides russh-based client for live metrics.

use serde::{Deserialize, Serialize};

/// Indicates where a host entry originated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HostSource {
    /// Imported from `~/.ssh/config` at startup.
    SshConfig,
    /// Added manually through the TUI form.
    #[default]
    Manual,
}

/// A single host entry used for SSH connections.
///
/// Populated either from `~/.ssh/config` (via the parser) or from
/// `~/.config/omnyssh/hosts.toml` (manual entries added through the TUI).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    /// Display name / alias (e.g. `"web-prod-1"`).
    pub name: String,
    /// Hostname or IP address to connect to.
    pub hostname: String,
    /// SSH user. Defaults to `"root"` when not specified.
    #[serde(default = "default_user")]
    pub user: String,
    /// SSH port. Defaults to `22` when not specified.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Path to the private key file (e.g. `~/.ssh/id_ed25519`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    /// Password for password-based authentication (not recommended, used for initial setup).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// ProxyJump host alias (for bastion / jump-host setups).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<String>,
    /// Organisational tags (e.g. `["production", "web"]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Free-text notes about this host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Where this entry came from.
    #[serde(default)]
    pub source: HostSource,
    /// Original host name from `~/.ssh/config` if this host was renamed.
    /// Used to prevent duplicate entries when a SSH-config host is renamed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_ssh_host: Option<String>,

    // -----------------------------------------------------------------------
    // Auto SSH Key Setup metadata
    // -----------------------------------------------------------------------
    /// Date when SSH key was configured by OmnySSH (ISO 8601 format).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_setup_date: Option<String>,
    /// Whether password authentication has been disabled on the server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_auth_disabled: Option<bool>,
}

fn default_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| String::from("root"))
}

fn default_port() -> u16 {
    22
}

impl Default for Host {
    fn default() -> Self {
        Self {
            name: String::new(),
            hostname: String::new(),
            user: default_user(),
            port: default_port(),
            identity_file: None,
            password: None,
            proxy_jump: None,
            tags: Vec::new(),
            notes: None,
            source: HostSource::default(),
            original_ssh_host: None,
            key_setup_date: None,
            password_auth_disabled: None,
        }
    }
}

impl Host {
    /// Returns the stable host identifier used as a key in metrics/status maps.
    ///
    /// Currently this is simply `host.name`. Using a method keeps callers
    /// decoupled from the implementation detail.
    pub fn id(&self) -> &str {
        &self.name
    }
}

/// Runtime connection status for a host. Never serialised to disk.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ConnectionStatus {
    /// No connection attempt has been made yet.
    #[default]
    Unknown,
    /// A connection attempt is currently in progress.
    Connecting,
    /// The host is connected.
    Connected,
    /// The last connection attempt failed with the given message.
    Failed(String),
}
