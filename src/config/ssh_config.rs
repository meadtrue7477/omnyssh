//! Parser for `~/.ssh/config`.
//!
//! Supported directives: `Host`, `HostName`, `User`, `Port`,
//! `IdentityFile`, `ProxyJump`, `Include`.
//!
//! The original file is **never modified**.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ssh::client::{Host, HostSource};

/// Parses the text of an SSH config file and returns all non-wildcard hosts.
///
/// `Host *` entries are silently skipped.
/// `Include` recursion is limited to 3 levels to prevent cycles.
pub fn parse_ssh_config(content: &str) -> Vec<Host> {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    parse_content(content, 0, &mut visited)
}

/// Loads and parses an SSH config file from disk.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn load_from_file(path: &Path) -> anyhow::Result<Vec<Host>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?;
    Ok(parse_ssh_config(&content))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn parse_content(content: &str, depth: usize, visited: &mut HashSet<PathBuf>) -> Vec<Host> {
    if depth > 3 {
        return Vec::new();
    }

    let mut hosts: Vec<Host> = Vec::new();
    let mut current: Option<Host> = None;
    // True when we are inside a wildcard `Host *` block (skip directives).
    let mut in_wildcard = false;

    for raw_line in content.lines() {
        let line = strip_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }

        let Some((keyword, value)) = split_kv(&line) else {
            continue;
        };

        match keyword.to_lowercase().as_str() {
            "host" => {
                // Flush previous host before starting a new block.
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                in_wildcard = value.contains('*') || value.contains('?');
                if !in_wildcard {
                    let h = Host {
                        name: value.to_string(),
                        source: HostSource::SshConfig,
                        ..Host::default()
                    };
                    current = Some(h);
                } else {
                    current = None;
                }
            }
            "hostname" if !in_wildcard => {
                if let Some(ref mut h) = current {
                    h.hostname = value.to_string();
                }
            }
            "user" if !in_wildcard => {
                if let Some(ref mut h) = current {
                    h.user = value.to_string();
                }
            }
            "port" if !in_wildcard => {
                if let Some(ref mut h) = current {
                    if let Ok(p) = value.parse::<u16>() {
                        h.port = p;
                    }
                }
            }
            "identityfile" if !in_wildcard => {
                if let Some(ref mut h) = current {
                    h.identity_file = Some(expand_tilde(value));
                }
            }
            "proxyjump" if !in_wildcard => {
                if let Some(ref mut h) = current {
                    h.proxy_jump = Some(value.to_string());
                }
            }
            "include" => {
                // Flush the current host before processing Include.
                if let Some(h) = current.take() {
                    hosts.push(h);
                }
                in_wildcard = false;
                let expanded = expand_tilde(value);
                for path in expand_include_glob(&expanded) {
                    // Canonicalise to catch cycles (symlinks, etc.).
                    let canonical = path.canonicalize().unwrap_or_else(|e| {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "canonicalize failed; symlink-cycle detection disabled for this path"
                        );
                        path.clone()
                    });
                    if !visited.insert(canonical) {
                        continue; // already visited — break cycle
                    }
                    match std::fs::read_to_string(&path) {
                        Ok(sub) => hosts.extend(parse_content(&sub, depth + 1, visited)),
                        Err(e) => {
                            tracing::warn!(path = %path.display(), error = %e, "Include file unreadable")
                        }
                    }
                }
            }
            _ => {} // Unknown directive — silently ignore.
        }
    }

    // Flush the last pending host.
    if let Some(h) = current.take() {
        hosts.push(h);
    }

    // Fallback: if HostName was never set, use the alias as the address.
    for h in &mut hosts {
        if h.hostname.is_empty() {
            h.hostname = h.name.clone();
        }
    }

    hosts
}

/// Removes everything from the first `#` onwards (inline comments).
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// Splits `"Keyword Value"` or `"Keyword=Value"` into `("Keyword", "Value")`.
fn split_kv(line: &str) -> Option<(&str, &str)> {
    // Split on the first whitespace or '='.
    let idx = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let keyword = line[..idx].trim();
    let value = line[idx + 1..].trim_start_matches('=').trim();
    if keyword.is_empty() || value.is_empty() {
        None
    } else {
        Some((keyword, value))
    }
}

/// Expands a leading `~/` to the user's home directory.
fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().into_owned();
        }
    }
    s.to_string()
}

/// Resolves an Include pattern to a list of file paths.
///
/// Supports a single `*` wildcard in the file-name component only.
/// The parent directory must exist; `*` in path segments other than the
/// last one is not supported (matches OpenSSH behaviour).
fn expand_include_glob(pattern: &str) -> Vec<PathBuf> {
    let path = PathBuf::from(pattern);
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let file_name = match path.file_name().and_then(|f| f.to_str()) {
        Some(n) => n.to_string(),
        None => return Vec::new(),
    };

    if !file_name.contains('*') {
        return if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        };
    }

    // Simple single-`*` glob: match prefix and suffix.
    let (prefix, suffix) = file_name.split_once('*').unwrap_or((&file_name, ""));
    match std::fs::read_dir(&parent) {
        Ok(entries) => {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with(prefix) && name.ends_with(suffix)
                })
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            paths.sort(); // deterministic order
            paths
        }
        Err(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        assert!(parse_ssh_config("").is_empty());
        assert!(parse_ssh_config("   \n  \n").is_empty());
    }

    #[test]
    fn test_comments_only() {
        let cfg = "# This is a comment\n# Another comment\n";
        assert!(parse_ssh_config(cfg).is_empty());
    }

    #[test]
    fn test_minimal_config() {
        let cfg = "\
Host myserver
    HostName 192.168.1.100
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "myserver");
        assert_eq!(hosts[0].hostname, "192.168.1.100");
        // default_user() falls back to $USER / $LOGNAME / "root"
        let expected_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| String::from("root"));
        assert_eq!(hosts[0].user, expected_user);
        assert_eq!(hosts[0].port, 22); // default
    }

    #[test]
    fn test_hostname_fallback_to_name() {
        // If HostName is omitted, hostname should equal the alias.
        let cfg = "\
Host myalias
    User admin
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, "myalias");
    }

    #[test]
    fn test_full_config_all_fields() {
        let cfg = "\
Host web-prod-1
    HostName 192.168.1.10
    User deploy
    Port 2222
    IdentityFile ~/.ssh/id_ed25519
    ProxyJump bastion
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        let h = &hosts[0];
        assert_eq!(h.name, "web-prod-1");
        assert_eq!(h.hostname, "192.168.1.10");
        assert_eq!(h.user, "deploy");
        assert_eq!(h.port, 2222);
        assert!(h
            .identity_file
            .as_deref()
            .unwrap_or("")
            .contains("id_ed25519"));
        assert_eq!(h.proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn test_multiple_hosts() {
        let cfg = "\
Host web
    HostName 10.0.0.1
    User ubuntu

Host db
    HostName 10.0.0.2
    User postgres
    Port 5432
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0].name, "web");
        assert_eq!(hosts[1].name, "db");
        assert_eq!(hosts[1].port, 5432);
    }

    #[test]
    fn test_wildcard_host_ignored() {
        let cfg = "\
Host *
    User ubuntu
    ServerAliveInterval 60

Host realhost
    HostName 10.0.0.1
";
        let hosts = parse_ssh_config(cfg);
        // Only the non-wildcard host should be imported.
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "realhost");
        // The wildcard User should NOT leak into realhost.
        // (We don't apply wildcard defaults — per plan, just skip them.)
    }

    #[test]
    fn test_proxy_jump() {
        let cfg = "\
Host bastion
    HostName jump.example.com
    User ops

Host internal
    HostName 192.168.100.50
    User admin
    ProxyJump bastion
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 2);
        let internal = hosts.iter().find(|h| h.name == "internal").unwrap();
        assert_eq!(internal.proxy_jump.as_deref(), Some("bastion"));
    }

    #[test]
    fn test_nonstandard_port() {
        let cfg = "Host custom\n    HostName 1.2.3.4\n    Port 22022\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts[0].port, 22022);
    }

    #[test]
    fn test_case_insensitive_keywords() {
        // OpenSSH config is case-insensitive for keywords.
        let cfg = "\
host server1
    hostname 10.0.0.1
    user admin
    port 2222
";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, "10.0.0.1");
        assert_eq!(hosts[0].user, "admin");
        assert_eq!(hosts[0].port, 2222);
    }

    #[test]
    fn test_inline_comment() {
        let cfg = "Host srv # this is a comment\n    HostName 1.2.3.4\n";
        let hosts = parse_ssh_config(cfg);
        // "srv # this is a comment" — name should be trimmed
        // Note: our strip_comment removes '#' and after, so name = "srv"
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "srv");
    }

    #[test]
    fn test_source_is_ssh_config() {
        let cfg = "Host test\n    HostName 1.2.3.4\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts[0].source, crate::ssh::client::HostSource::SshConfig);
    }

    #[test]
    fn test_equals_separator() {
        // Some configs use '=' instead of space.
        let cfg = "Host=myhost\n    HostName=10.0.0.1\n    User=admin\n";
        let hosts = parse_ssh_config(cfg);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].hostname, "10.0.0.1");
        assert_eq!(hosts[0].user, "admin");
    }
}
