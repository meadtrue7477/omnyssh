//! Application configuration modules.
//!
//! - [`app_config`]  — main `~/.config/omnyssh/config.toml`
//! - [`ssh_config`]  — parser for `~/.ssh/config`
//! - [`snippets`]    — `~/.config/omnyssh/snippets.toml`
//!
//! Top-level functions in this module handle loading and persisting the
//! host list (`hosts.toml`).

pub mod app_config;
pub mod snippets;
pub mod ssh_config;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::ssh::client::{Host, HostSource};
use crate::utils::platform;

// ---------------------------------------------------------------------------
// hosts.toml I/O
// ---------------------------------------------------------------------------

/// TOML container for the hosts list.
#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default)]
    hosts: Vec<Host>,
}

/// Loads manually-added hosts from `~/.config/omnyssh/hosts.toml`.
///
/// Returns an empty `Vec` if the file does not exist yet.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_hosts() -> anyhow::Result<Vec<Host>> {
    let path = platform::hosts_config_path().context("Cannot determine hosts config path")?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let file: HostsFile =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;

    Ok(file.hosts)
}

/// Persists the manually-added hosts (source == `Manual`) to
/// `~/.config/omnyssh/hosts.toml`.
///
/// SSH-config-derived hosts are intentionally **not** written — they are
/// re-imported from `~/.ssh/config` on every startup.
///
/// # Errors
/// Returns an error if the directory cannot be created or the file cannot
/// be written.
pub fn save_hosts(hosts: &[Host]) -> anyhow::Result<()> {
    let dir = platform::app_config_dir().context("Cannot determine app config directory")?;

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create directory {}", dir.display()))?;

    let path = dir.join("hosts.toml");

    let manual: Vec<Host> = hosts
        .iter()
        .filter(|h| h.source == HostSource::Manual)
        .cloned()
        .collect();

    let file = HostsFile { hosts: manual };
    let content = toml::to_string_pretty(&file).context("Failed to serialise hosts")?;

    // Write to a temp file and rename for atomic replacement (avoids a corrupt
    // hosts.toml if the process is interrupted mid-write).
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

/// Loads all hosts: manually-added (`hosts.toml`) merged with hosts
/// imported from `~/.ssh/config`.
///
/// Manual entries take priority over SSH-config entries with the same name.
/// SSH-config entries are appended after all manual ones.
///
/// # Errors
/// Returns an error if `hosts.toml` exists but is unreadable/malformed.
/// A missing or unreadable `~/.ssh/config` is silently ignored.
pub fn load_all_hosts() -> anyhow::Result<Vec<Host>> {
    // 1. Manual hosts (from hosts.toml).
    let manual = load_hosts()?;

    // 2. Hosts from ~/.ssh/config.
    let mut ssh_hosts: Vec<Host> = Vec::new();
    if let Some(ssh_path) = platform::ssh_config_path() {
        if ssh_path.exists() {
            match ssh_config::load_from_file(&ssh_path) {
                Ok(h) => ssh_hosts = h,
                Err(e) => tracing::warn!("SSH config parse error: {}", e),
            }
        }
    }

    // 3. Merge: manual names take priority.
    // Also exclude SSH config hosts that have been renamed (tracked via original_ssh_host).
    let manual_names: std::collections::HashSet<String> =
        manual.iter().map(|h| h.name.clone()).collect();

    // Build set of original SSH config hostnames that have been renamed
    let renamed_ssh_hosts: std::collections::HashSet<String> = manual
        .iter()
        .filter_map(|h| h.original_ssh_host.clone())
        .collect();

    let mut all = manual;
    for h in ssh_hosts {
        // Skip if a manual host already uses this name, or if this SSH host was renamed
        if !manual_names.contains(&h.name) && !renamed_ssh_hosts.contains(&h.name) {
            all.push(h);
        }
    }

    Ok(all)
}
