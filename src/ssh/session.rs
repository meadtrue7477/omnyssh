//! Async SSH session management via russh.
//!
//! Provides [`SshSession`] — a thin wrapper around a russh client handle that
//! supports connecting, executing commands, and graceful disconnect.
//! Authentication order: identity file → SSH agent → failure.
//!
//! Connection and command timeouts are enforced:
//! - Connect timeout: 10 seconds
//! - Command timeout: 30 seconds

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use russh::client::{self, Handle};
use russh::ChannelMsg;
use tokio::time;

use crate::ssh::client::Host;

// ---------------------------------------------------------------------------
// russh Handler implementation
// ---------------------------------------------------------------------------

/// Minimal russh client handler for metric collection.
///
/// Verifies the server's host key against `~/.ssh/known_hosts`.
/// Unknown hosts (first connection) are accepted; changed keys are rejected.
struct MetricsHandler {
    /// Hostname used for known_hosts lookup.
    host: String,
    /// Port used for known_hosts lookup.
    port: u16,
}

#[async_trait]
impl client::Handler for MetricsHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        match russh::keys::check_known_hosts(&self.host, self.port, server_public_key) {
            Ok(true) => Ok(true),  // key is in known_hosts and matches
            Ok(false) => Ok(true), // host unknown — accept (first-connection semantics)
            Err(russh::keys::Error::KeyChanged { .. }) => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    "Server key mismatch in known_hosts — possible MITM attack, refusing connection"
                );
                Ok(false)
            }
            Err(e) => {
                tracing::warn!(error = %e, "known_hosts check failed; accepting key");
                Ok(true)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SshSession
// ---------------------------------------------------------------------------

/// An authenticated SSH session ready for command execution.
///
/// Holds the russh client handle for the duration of its lifetime.
/// Drop → the connection is cleaned up by russh's internal tasks.
///
/// Wrapped in Arc to allow sharing across multiple operations (discovery + metrics).
#[derive(Clone)]
pub struct SshSession {
    handle: Arc<Handle<MetricsHandler>>,
}

impl SshSession {
    /// Connect and authenticate to `host`.
    ///
    /// Authentication is attempted in order:
    /// 1. SSH agent (unix only, via `SSH_AUTH_SOCK`).
    /// 2. Identity file specified in the host config (`identity_file`).
    /// 3. Default key files (`~/.ssh/id_ed25519`, `id_rsa`, etc.).
    /// 4. Password (if provided in host config).
    ///
    /// Returns an error when no method succeeds or the connection times out.
    ///
    /// # Errors
    /// - Connection timeout (> 10 s)
    /// - Authentication failure
    /// - Network error
    pub async fn connect(host: &Host) -> anyhow::Result<Self> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(30)),
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });

        let addr = format!("{}:{}", host.hostname, host.port);
        let mut handle = time::timeout(
            Duration::from_secs(10),
            client::connect(
                config,
                addr,
                MetricsHandler {
                    host: host.hostname.clone(),
                    port: host.port,
                },
            ),
        )
        .await
        .map_err(|_| anyhow!("SSH connection timed out (10 s)"))?
        .context("SSH connection failed")?;

        let authenticated = authenticate(&mut handle, host).await?;
        if !authenticated {
            return Err(anyhow!("SSH authentication failed for {}", host.name));
        }

        Ok(Self {
            handle: Arc::new(handle),
        })
    }

    /// Execute a shell command on the remote host and return its stdout.
    ///
    /// A new SSH channel is opened for each call so sessions can be
    /// reused across multiple commands.
    ///
    /// # Errors
    /// Returns an error on channel failure or if the command times out (30 s).
    pub async fn run_command(&self, cmd: &str) -> anyhow::Result<String> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SSH channel")?;

        channel.exec(true, cmd).await.context("exec SSH command")?;

        let output = time::timeout(Duration::from_secs(30), collect_output(&mut channel))
            .await
            .map_err(|_| anyhow!("command timed out (30 s): {}", cmd))?
            .context("read command output")?;

        Ok(output)
    }

    /// Opens a new SSH channel, requests the SFTP subsystem, and returns the
    /// channel as an async stream suitable for [`russh_sftp::client::SftpSession::new`].
    ///
    /// The `SshSession` **must** remain alive for the entire lifetime of the
    /// SFTP session — dropping it closes the underlying TCP connection.
    ///
    /// # Errors
    /// Returns an error if the channel cannot be opened or if the server rejects
    /// the SFTP subsystem request.
    pub async fn open_sftp_channel(
        &self,
    ) -> anyhow::Result<russh::ChannelStream<russh::client::Msg>> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .context("open SFTP session channel")?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .context("request SFTP subsystem")?;
        Ok(channel.into_stream())
    }

    /// Gracefully close the SSH connection.
    pub async fn disconnect(self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

// ---------------------------------------------------------------------------
// Authentication helpers
// ---------------------------------------------------------------------------

async fn authenticate(handle: &mut Handle<MetricsHandler>, host: &Host) -> anyhow::Result<bool> {
    let user = host.user.clone();

    // 1. Try SSH agent first — it handles passphrase-protected keys and is the
    //    most common auth method for non-interactive clients.
    #[cfg(unix)]
    {
        if try_agent_auth(handle, &user).await.unwrap_or(false) {
            return Ok(true);
        }
    }

    // 2. Try explicit identity_file from host config.
    if let Some(key_path) = &host.identity_file {
        let path = expand_tilde(key_path);
        if try_key_auth(handle, &user, &path).await.unwrap_or(false) {
            return Ok(true);
        }
    }

    // 3. Try default key files — mirrors what the `ssh` binary does when no
    //    -i flag is given. Skips files that don't exist.
    for key_path in default_key_paths() {
        if key_path.exists() {
            let path_str = key_path.to_string_lossy().into_owned();
            if try_key_auth(handle, &user, &path_str)
                .await
                .unwrap_or(false)
            {
                return Ok(true);
            }
        }
    }

    // 4. Try password authentication if provided.
    //    Password auth is NOT recommended for production use but is required for
    //    the initial connection before setting up key-based auth.
    if let Some(password) = &host.password {
        if try_password_auth(handle, &user, password)
            .await
            .unwrap_or(false)
        {
            tracing::info!(
                host = %host.name,
                "Connected via password authentication — consider setting up SSH key"
            );
            return Ok(true);
        }
    }

    Ok(false)
}

/// Returns the standard default SSH private key paths in priority order.
fn default_key_paths() -> Vec<std::path::PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    let ssh = home.join(".ssh");
    [
        "id_ed25519",
        "id_rsa",
        "id_ecdsa",
        "id_ecdsa_sk",
        "id_ed25519_sk",
        "id_dsa",
    ]
    .iter()
    .map(|name| ssh.join(name))
    .collect()
}

async fn try_key_auth(
    handle: &mut Handle<MetricsHandler>,
    user: &str,
    key_path: &str,
) -> anyhow::Result<bool> {
    // load_secret_key is synchronous (file I/O) — offload to blocking pool.
    let path = key_path.to_string();
    let key_pair = tokio::task::spawn_blocking(move || {
        russh::keys::load_secret_key(&path, None).with_context(|| format!("load key from {path}"))
    })
    .await
    .context("spawn_blocking panicked")??;

    let ok = handle
        .authenticate_publickey(user, Arc::new(key_pair))
        .await
        .context("authenticate_publickey")?;
    Ok(ok)
}

#[cfg(unix)]
async fn try_agent_auth(handle: &mut Handle<MetricsHandler>, user: &str) -> anyhow::Result<bool> {
    use russh::keys::agent::client::AgentClient;

    let mut agent = AgentClient::connect_env()
        .await
        .context("connect to SSH agent")?;

    let identities = agent
        .request_identities()
        .await
        .context("request agent identities")?;

    for pubkey in identities {
        let (agent_back, result) = handle.authenticate_future(user, pubkey, agent).await;
        agent = agent_back;
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => continue,
            Err(_) => continue,
        }
    }
    Ok(false)
}

/// Try password-based authentication.
///
/// # Errors
/// Returns an error if the authentication attempt fails.
async fn try_password_auth(
    handle: &mut Handle<MetricsHandler>,
    user: &str,
    password: &str,
) -> anyhow::Result<bool> {
    let ok = handle
        .authenticate_password(user, password)
        .await
        .context("authenticate with password")?;
    Ok(ok)
}

// ---------------------------------------------------------------------------
// Output collection
// ---------------------------------------------------------------------------

async fn collect_output(
    channel: &mut russh::Channel<russh::client::Msg>,
) -> anyhow::Result<String> {
    let mut buf = Vec::new();
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Data { ref data }) => {
                buf.extend_from_slice(data);
            }
            Some(ChannelMsg::ExtendedData { .. }) => {
                // stderr — discard to avoid corrupting stdout-only parser input
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) => break,
            Some(ChannelMsg::ExitStatus { .. }) => break,
            None => break,
            _ => {}
        }
    }
    // Use .lines() semantics: replace \r\n → \n for cross-platform safety.
    let raw = String::from_utf8_lossy(&buf);
    let normalised: String = raw.lines().flat_map(|l| [l, "\n"]).collect();
    Ok(normalised)
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        if let Some(home) = dirs::home_dir() {
            return path.replacen('~', &home.to_string_lossy(), 1);
        }
    }
    path.to_string()
}
