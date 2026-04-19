//! Auto SSH Key Setup
//!
//! Provides automated SSH key generation and server configuration for transitioning
//! from password-based to key-based authentication.
//!
//! ## Safety Invariants
//! - Never disable password authentication without verified key auth
//! - Append to authorized_keys, never overwrite
//! - Always backup sshd_config before modification
//! - Show warning about alternative access before disabling password
//! - Log all operations to key_setup.log
//! - Private key 600, .ssh directory 700
//! - Never transmit private key over network

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time;
use tracing::{error, info, warn};

use crate::ssh::client::Host;
use crate::ssh::session::SshSession;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum length for sanitized hostname in key filename.
const MAX_HOSTNAME_LENGTH: usize = 64;

/// Total timeout for the entire key setup process.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// Timeout for individual SSH operations during key setup.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

// ---------------------------------------------------------------------------
// Key Type
// ---------------------------------------------------------------------------

/// Supported SSH key types for generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// Ed25519 (recommended, modern, fast).
    Ed25519,
    /// RSA 4096-bit (compatibility with older systems).
    Rsa4096,
}

impl KeyType {
    /// Returns the file extension for this key type.
    pub fn extension(&self) -> &'static str {
        match self {
            KeyType::Ed25519 => "ed25519",
            KeyType::Rsa4096 => "rsa",
        }
    }
}

// ---------------------------------------------------------------------------
// Key Setup Steps
// ---------------------------------------------------------------------------

/// Individual steps in the key setup process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeySetupStep {
    GenerateKey = 1,
    CopyPublicKey = 2,
    VerifyKeyAuth = 3,
    DisablePassword = 4,
    ReloadSshd = 5,
    FinalCheck = 6,
}

impl KeySetupStep {
    /// Returns all steps in order.
    pub fn all_steps() -> Vec<Self> {
        vec![
            Self::GenerateKey,
            Self::CopyPublicKey,
            Self::VerifyKeyAuth,
            Self::DisablePassword,
            Self::ReloadSshd,
            Self::FinalCheck,
        ]
    }

    /// Human-readable description for UI display.
    pub fn description(&self) -> &'static str {
        match self {
            Self::GenerateKey => "Generating Ed25519 key pair",
            Self::CopyPublicKey => "Copying public key to server",
            Self::VerifyKeyAuth => "Verifying key authentication",
            Self::DisablePassword => "Disabling password authentication",
            Self::ReloadSshd => "Reloading SSH service",
            Self::FinalCheck => "Final verification",
        }
    }
}

// ---------------------------------------------------------------------------
// Key Setup State
// ---------------------------------------------------------------------------

/// Result of the key setup process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySetupState {
    /// Setup not yet started.
    NotStarted,
    /// Setup is in progress.
    InProgress,
    /// Setup completed successfully (password auth disabled).
    Success,
    /// Setup partially succeeded (key works, but no sudo to disable password).
    PartialSuccess,
    /// Setup failed safely (password auth NOT disabled).
    FailedSafe,
    /// Setup failed after disabling password — needs rollback.
    NeedsRollback,
    /// Rollback completed.
    RolledBack,
}

/// State machine for tracking key setup progress.
#[derive(Debug)]
pub struct KeySetupMachine {
    state: KeySetupState,
    current_step: Option<KeySetupStep>,
    has_sudo: bool,
    password_disabled: bool,
}

impl KeySetupMachine {
    /// Creates a new state machine in the NotStarted state.
    pub fn new() -> Self {
        Self {
            state: KeySetupState::NotStarted,
            current_step: None,
            has_sudo: true,
            password_disabled: false,
        }
    }

    /// Returns the current state.
    pub fn state(&self) -> &KeySetupState {
        &self.state
    }

    /// Returns whether password authentication has been disabled.
    pub fn password_disabled(&self) -> bool {
        self.password_disabled
    }

    /// Sets the sudo availability flag.
    pub fn set_has_sudo(&mut self, has_sudo: bool) {
        self.has_sudo = has_sudo;
    }

    /// Marks a step as complete with the given result.
    ///
    /// Updates the state machine based on which step completed and whether it succeeded.
    /// Implements the safety invariants from tech-2.md B.3.3.
    pub fn step_result(&mut self, step: KeySetupStep, result: Result<()>) {
        self.current_step = Some(step);

        match (step, result) {
            // Step 1-2: Safe to fail, no changes to server yet.
            (KeySetupStep::GenerateKey | KeySetupStep::CopyPublicKey, Err(_)) => {
                self.state = KeySetupState::FailedSafe;
            }

            // Step 3 (VerifyKeyAuth): CRITICAL — if this fails, STOP.
            // Never disable password without verified key.
            (KeySetupStep::VerifyKeyAuth, Err(_)) => {
                self.state = KeySetupState::FailedSafe;
            }
            (KeySetupStep::VerifyKeyAuth, Ok(())) if !self.has_sudo => {
                // Key works, but no sudo — partial success.
                self.state = KeySetupState::PartialSuccess;
            }

            // Step 4 (DisablePassword): Point of no return.
            (KeySetupStep::DisablePassword, Ok(())) => {
                self.password_disabled = true;
            }
            (KeySetupStep::DisablePassword, Err(_)) => {
                // Failed to disable password — safe, stop here.
                self.state = KeySetupState::FailedSafe;
            }

            // Step 5 (ReloadSshd): Mostly safe (reload doesn't kill existing connections).
            (KeySetupStep::ReloadSshd, Err(_)) => {
                // Reload failed, but password is already disabled.
                // This might be okay if the daemon auto-reloaded, but risky.
                self.state = KeySetupState::NeedsRollback;
            }

            // Step 6 (FinalCheck): Verify key still works after reload.
            // If this fails, password is disabled but key doesn't work — emergency rollback!
            (KeySetupStep::FinalCheck, Err(_)) => {
                self.state = KeySetupState::NeedsRollback;
            }
            (KeySetupStep::FinalCheck, Ok(())) => {
                self.state = KeySetupState::Success;
            }

            // All other OK results → continue.
            (_, Ok(())) => {
                self.state = KeySetupState::InProgress;
            }
        }
    }

    /// Marks the rollback as complete.
    pub fn rollback_complete(&mut self) {
        self.state = KeySetupState::RolledBack;
        self.password_disabled = false;
    }
}

impl Default for KeySetupMachine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Key Generation
// ---------------------------------------------------------------------------

/// Generates an Ed25519 SSH key pair and writes it to disk.
///
/// Returns the paths to the private and public key files.
///
/// # Errors
/// Returns an error if key generation or file I/O fails.
///
/// # Safety
/// - Private key is written with mode 0600.
/// - .ssh directory is created with mode 0700 if it doesn't exist.
pub async fn generate_key_pair(host_name: &str, key_type: KeyType) -> Result<(PathBuf, PathBuf)> {
    let sanitized = sanitize_hostname(host_name);
    let key_filename = format!("omnyssh_{}_{}", sanitized, key_type.extension());

    let ssh_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("Cannot determine home directory"))?
        .join(".ssh");

    // Create .ssh directory if it doesn't exist.
    tokio::fs::create_dir_all(&ssh_dir)
        .await
        .with_context(|| format!("Failed to create {}", ssh_dir.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        tokio::fs::set_permissions(&ssh_dir, perms)
            .await
            .with_context(|| format!("Failed to set permissions on {}", ssh_dir.display()))?;
    }

    let private_key_path = ssh_dir.join(&key_filename);
    let public_key_path = ssh_dir.join(format!("{}.pub", key_filename));

    // Check if key already exists - if so, reuse it instead of failing.
    if private_key_path.exists() && public_key_path.exists() {
        info!(
            "Key already exists for {}, reusing: {}",
            host_name,
            private_key_path.display()
        );
        return Ok((private_key_path, public_key_path));
    }

    // Generate the key pair using ssh-keygen directly to ensure macOS OpenSSH compatibility.
    // Using ssh-keygen ensures the key is in the correct format (OpenSSH) from the start.
    info!(
        "Generating {} key pair for {}",
        key_type.extension(),
        host_name
    );

    let key_type_arg = match key_type {
        KeyType::Ed25519 => "ed25519",
        KeyType::Rsa4096 => "rsa",
    };

    let mut keygen_cmd = tokio::process::Command::new("ssh-keygen");
    keygen_cmd
        .arg("-t")
        .arg(key_type_arg)
        .arg("-f")
        .arg(&private_key_path)
        .arg("-N")
        .arg("") // No passphrase
        .arg("-C")
        .arg(format!("omnyssh-{}", host_name)); // Comment

    // For RSA, specify 4096 bits
    if matches!(key_type, KeyType::Rsa4096) {
        keygen_cmd.arg("-b").arg("4096");
    }

    let keygen_output = keygen_cmd
        .output()
        .await
        .context("Failed to run ssh-keygen for key generation")?;

    if !keygen_output.status.success() {
        let stderr = String::from_utf8_lossy(&keygen_output.stderr);
        return Err(anyhow!("ssh-keygen failed to generate key: {}", stderr));
    }

    // Ensure private key has correct permissions (ssh-keygen should set this, but be explicit)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        tokio::fs::set_permissions(&private_key_path, perms)
            .await
            .with_context(|| {
                format!(
                    "Failed to set permissions on private key {}",
                    private_key_path.display()
                )
            })?;
    }

    info!(
        "Generated key pair:\n  Private: {}\n  Public: {}",
        private_key_path.display(),
        public_key_path.display()
    );

    Ok((private_key_path, public_key_path))
}

/// Sanitizes a hostname for use in a key filename.
///
/// - Replaces non-alphanumeric characters (except `-`, `_`, `.`) with `_`
/// - Truncates to MAX_HOSTNAME_LENGTH
/// - Returns "unnamed_host" for empty input
///
/// # Examples
/// ```
/// use omnyssh::ssh::key_setup::sanitize_hostname;
///
/// assert_eq!(sanitize_hostname("web-prod-1"), "web-prod-1");
/// assert_eq!(sanitize_hostname("my server (prod)"), "my_server__prod_");
/// assert_eq!(sanitize_hostname("../../etc/passwd"), "______etc_passwd");
/// assert_eq!(sanitize_hostname(""), "unnamed_host");
/// ```
pub fn sanitize_hostname(hostname: &str) -> String {
    if hostname.is_empty() {
        return "unnamed_host".to_string();
    }

    let sanitized: String = hostname
        .chars()
        .map(|c| {
            // Only allow alphanumerics, hyphens, and underscores.
            // Dots are replaced to prevent path traversal attacks (../../etc/passwd).
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_HOSTNAME_LENGTH)
        .collect();

    if sanitized.is_empty() {
        "unnamed_host".to_string()
    } else {
        sanitized
    }
}

/// Returns the key file path that would be used for a given host.
pub fn key_path_for_host(host_name: &str, key_type: KeyType) -> PathBuf {
    let sanitized = sanitize_hostname(host_name);
    let key_filename = format!("omnyssh_{}_{}", sanitized, key_type.extension());

    dirs::home_dir()
        .expect("Cannot determine home directory")
        .join(".ssh")
        .join(key_filename)
}

// ---------------------------------------------------------------------------
// SSH Command Builders
// ---------------------------------------------------------------------------

/// Builds the command to append a public key to authorized_keys.
///
/// The command creates ~/.ssh if it doesn't exist, appends the key (never overwrites),
/// and sets correct permissions.
pub fn build_authorized_keys_command(public_key: &str) -> String {
    // Escape single quotes in the public key.
    let escaped_key = public_key.replace('\'', "'\\''");

    format!(
        r#"mkdir -p ~/.ssh && chmod 700 ~/.ssh && \
           echo '{escaped_key}' >> ~/.ssh/authorized_keys && \
           chmod 600 ~/.ssh/authorized_keys"#
    )
}

/// Builds the command to disable password authentication in sshd_config.
///
/// The command:
/// - Checks for sudo access
/// - Creates a timestamped backup of sshd_config
/// - Comments out Include directives to prevent overrides from sshd_config.d/*
/// - Disables password authentication (PasswordAuthentication, ChallengeResponseAuthentication, KbdInteractiveAuthentication)
/// - Disables UsePAM to prevent PAM from bypassing password auth restrictions
/// - Validates the config with `sshd -t`
///
/// ## Security Note
/// Even with `PasswordAuthentication no`, PAM (Pluggable Authentication Modules) can provide
/// alternative authentication methods (keyboard-interactive) that accept passwords.
/// Setting `UsePAM no` ensures password authentication is completely disabled and cannot be
/// bypassed with flags like `ssh -o PubkeyAuthentication=no`.
///
/// ## Include Directive Handling
/// Many cloud providers (AWS, DigitalOcean, etc.) use `/etc/ssh/sshd_config.d/*.conf` files
/// (e.g., `50-cloud-init.conf`) that override the main config. We comment out the Include
/// directive to prevent these files from re-enabling password authentication.
///
/// Returns the command string or an error if sudo is required but unavailable.
pub fn build_disable_password_command() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");

    format!(
        r#"sudo -n true 2>/dev/null || {{ echo "OMNYSSH_NO_SUDO"; exit 1; }}; \
           sudo cp /etc/ssh/sshd_config /etc/ssh/sshd_config.omnyssh_backup.{timestamp} && \
           sudo sed -i.bak 's/^Include\s/#Include /' /etc/ssh/sshd_config && \
           sudo sed -i.bak 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config && \
           sudo sed -i.bak 's/^#\?ChallengeResponseAuthentication.*/ChallengeResponseAuthentication no/' /etc/ssh/sshd_config && \
           sudo sed -i.bak 's/^#\?KbdInteractiveAuthentication.*/KbdInteractiveAuthentication no/' /etc/ssh/sshd_config && \
           sudo sed -i.bak 's/^#\?UsePAM.*/UsePAM no/' /etc/ssh/sshd_config && \
           sudo sshd -t || {{ echo "OMNYSSH_CONFIG_ERROR"; sudo cp /etc/ssh/sshd_config.omnyssh_backup.{timestamp} /etc/ssh/sshd_config; exit 1; }}"#
    )
}

/// Builds the command to reload the SSH daemon.
///
/// Uses `reload` instead of `restart` to avoid killing existing connections.
pub fn build_reload_sshd_command() -> String {
    r#"if command -v systemctl &>/dev/null; then \
           sudo systemctl reload sshd 2>/dev/null || sudo systemctl reload ssh 2>/dev/null; \
       elif command -v service &>/dev/null; then \
           sudo service sshd reload 2>/dev/null || sudo service ssh reload 2>/dev/null; \
       else \
           echo "OMNYSSH_NO_INIT_SYSTEM"; exit 1; \
       fi"#
    .to_string()
}

/// Builds the emergency rollback command.
///
/// Restores the most recent OmnySSH backup of sshd_config and reloads the daemon.
pub fn build_rollback_command() -> String {
    r#"BACKUP=$(ls -t /etc/ssh/sshd_config.omnyssh_backup.* 2>/dev/null | head -1); \
       if [ -n "$BACKUP" ]; then \
           sudo cp "$BACKUP" /etc/ssh/sshd_config && \
           (sudo systemctl reload sshd 2>/dev/null || sudo systemctl reload ssh 2>/dev/null || sudo service sshd reload 2>/dev/null || sudo service ssh reload); \
       else \
           echo "OMNYSSH_NO_BACKUP"; exit 1; \
       fi"#
    .to_string()
}

// ---------------------------------------------------------------------------
// High-Level Key Setup Orchestrator
// ---------------------------------------------------------------------------

/// Executes the complete key setup process for a host.
///
/// This is the main entry point for Auto SSH Key Setup. It orchestrates all 6 steps
/// with proper error handling, timeouts, and rollback on failure.
///
/// # Errors
/// Returns an error if any critical step fails. Password authentication is never
/// disabled unless key authentication has been verified.
pub async fn setup_key_for_host(
    host: &Host,
    password_session: &SshSession,
    key_type: KeyType,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<KeySetupResult> {
    let mut machine = KeySetupMachine::new();
    let mut result = KeySetupResult {
        key_path: PathBuf::new(),
        state: KeySetupState::NotStarted,
        error_message: None,
    };

    // Wrap the entire process in a timeout.
    match time::timeout(
        TOTAL_TIMEOUT,
        setup_key_internal(host, password_session, key_type, &mut machine, progress_tx),
    )
    .await
    {
        Ok(Ok(key_path)) => {
            result.key_path = key_path;
            result.state = machine.state().clone();
            Ok(result)
        }
        Ok(Err(e)) => {
            error!("Key setup failed for {}: {}", host.name, e);
            result.state = machine.state().clone();
            result.error_message = Some(format!("{:#}", e));

            // Attempt rollback if needed.
            if matches!(machine.state(), KeySetupState::NeedsRollback) {
                if let Err(rollback_err) = emergency_rollback(password_session).await {
                    error!("Rollback failed: {}", rollback_err);
                    result.error_message = Some(format!(
                        "Setup failed AND rollback failed: {}\nRollback error: {}",
                        e, rollback_err
                    ));
                } else {
                    machine.rollback_complete();
                    result.state = KeySetupState::RolledBack;
                }
            }

            Err(e)
        }
        Err(_) => {
            let err = anyhow!(
                "Key setup timed out after {} seconds",
                TOTAL_TIMEOUT.as_secs()
            );
            result.error_message = Some(err.to_string());
            result.state = KeySetupState::FailedSafe;
            Err(err)
        }
    }
}

/// Internal implementation of the key setup process.
async fn setup_key_internal(
    host: &Host,
    password_session: &SshSession,
    key_type: KeyType,
    machine: &mut KeySetupMachine,
    progress_tx: Option<tokio::sync::mpsc::Sender<KeySetupStep>>,
) -> Result<PathBuf> {
    // Step 1: Generate key pair.
    info!("Step 1/6: Generating key pair for {}", host.name);
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::GenerateKey).await;
    }
    let (private_key_path, public_key_path) = match generate_key_pair(&host.name, key_type).await {
        Ok(paths) => {
            machine.step_result(KeySetupStep::GenerateKey, Ok(()));
            paths
        }
        Err(e) => {
            machine.step_result(KeySetupStep::GenerateKey, Err(anyhow!("Generation failed")));
            return Err(e);
        }
    };

    // Step 2: Copy public key to server.
    info!("Step 2/6: Copying public key to server");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::CopyPublicKey).await;
    }
    let public_key_content = tokio::fs::read_to_string(&public_key_path)
        .await
        .context("Failed to read public key file")?;

    let copy_cmd = build_authorized_keys_command(&public_key_content);
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&copy_cmd)).await {
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Err(anyhow!("Copy failed")));
            return Err(anyhow!("Failed to copy public key to server: {}", e));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::CopyPublicKey, Err(anyhow!("Copy failed")));
            return Err(anyhow!("Failed to copy public key to server: timeout"));
        }
    }

    // Step 3: Verify key authentication (CRITICAL).
    info!("Step 3/6: Verifying key authentication");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::VerifyKeyAuth).await;
    }
    let mut test_host = host.clone();
    test_host.identity_file = Some(private_key_path.to_string_lossy().to_string());
    test_host.password = None; // Force key-only auth.

    match time::timeout(STEP_TIMEOUT, SshSession::connect(&test_host)).await {
        Ok(Ok(test_session)) => {
            info!("Key authentication verified successfully!");
            test_session.disconnect().await;
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::VerifyKeyAuth, Err(anyhow!("Verify failed")));
            return Err(anyhow!(
                "Key authentication verification failed: {}. Password NOT disabled.",
                e
            ));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::VerifyKeyAuth, Err(anyhow!("Verify failed")));
            return Err(anyhow!(
                "Key authentication verification timed out. Password NOT disabled."
            ));
        }
    }

    // Check sudo availability.
    info!("Checking sudo availability");
    match password_session
        .run_command("sudo -n true 2>/dev/null")
        .await
    {
        Ok(_) => {
            info!("Sudo access confirmed");
        }
        Err(_) => {
            warn!("No sudo access — password authentication will NOT be disabled");
            machine.set_has_sudo(false);
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(())); // Trigger PartialSuccess.
            return Ok(private_key_path);
        }
    }

    // Step 4: Disable password authentication.
    info!("Step 4/6: Disabling password authentication");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::DisablePassword).await;
    }
    let disable_cmd = build_disable_password_command();
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&disable_cmd)).await {
        Ok(Ok(output)) if output.contains("OMNYSSH_NO_SUDO") => {
            machine.set_has_sudo(false);
            machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
            return Ok(private_key_path);
        }
        Ok(Ok(output)) if output.contains("OMNYSSH_CONFIG_ERROR") => {
            machine.step_result(KeySetupStep::DisablePassword, Err(anyhow!("Config error")));
            return Err(anyhow!("sshd config validation failed. Backup restored."));
        }
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::DisablePassword, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(
                KeySetupStep::DisablePassword,
                Err(anyhow!("Disable failed")),
            );
            return Err(anyhow!("Failed to disable password authentication: {}", e));
        }
        Err(_) => {
            machine.step_result(
                KeySetupStep::DisablePassword,
                Err(anyhow!("Disable failed")),
            );
            return Err(anyhow!(
                "Failed to disable password authentication: timeout"
            ));
        }
    }

    // Step 5: Reload sshd.
    info!("Step 5/6: Reloading SSH daemon");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::ReloadSshd).await;
    }
    let reload_cmd = build_reload_sshd_command();
    match time::timeout(STEP_TIMEOUT, password_session.run_command(&reload_cmd)).await {
        Ok(Ok(_)) => {
            machine.step_result(KeySetupStep::ReloadSshd, Ok(()));
        }
        Ok(Err(e)) => {
            machine.step_result(KeySetupStep::ReloadSshd, Err(anyhow!("Reload failed")));
            return Err(anyhow!("Failed to reload SSH daemon: {}", e));
        }
        Err(_) => {
            machine.step_result(KeySetupStep::ReloadSshd, Err(anyhow!("Reload failed")));
            return Err(anyhow!("Failed to reload SSH daemon: timeout"));
        }
    }

    // Step 6: Final check — verify key still works after reload.
    info!("Step 6/6: Final verification");
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(KeySetupStep::FinalCheck).await;
    }
    match time::timeout(STEP_TIMEOUT, SshSession::connect(&test_host)).await {
        Ok(Ok(final_session)) => {
            info!("Final verification passed! Key setup complete.");
            final_session.disconnect().await;
            machine.step_result(KeySetupStep::FinalCheck, Ok(()));
            Ok(private_key_path)
        }
        Ok(Err(e)) => {
            error!(
                "Final check failed: {}. Password is disabled but key doesn't work!",
                e
            );
            machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("Final check failed")));
            Err(anyhow!(
                "Final verification failed after disabling password. Attempting rollback."
            ))
        }
        Err(_) => {
            error!("Final check timed out. Password is disabled but key doesn't work!");
            machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("Final check failed")));
            Err(anyhow!(
                "Final verification timed out after disabling password. Attempting rollback."
            ))
        }
    }
}

/// Attempts to rollback sshd_config to the most recent OmnySSH backup.
async fn emergency_rollback(session: &SshSession) -> Result<()> {
    warn!("Attempting emergency rollback of sshd_config");
    let rollback_cmd = build_rollback_command();

    match time::timeout(STEP_TIMEOUT, session.run_command(&rollback_cmd)).await {
        Ok(Ok(output)) if output.contains("OMNYSSH_NO_BACKUP") => {
            Err(anyhow!("No backup file found for rollback"))
        }
        Ok(Ok(_)) => {
            info!("Rollback successful — password authentication restored");
            Ok(())
        }
        Ok(Err(e)) => Err(anyhow!("Rollback failed: {}", e)),
        Err(_) => Err(anyhow!("Rollback failed: timeout")),
    }
}

// ---------------------------------------------------------------------------
// Result Type
// ---------------------------------------------------------------------------

/// Result of the key setup process.
#[derive(Debug, Clone)]
pub struct KeySetupResult {
    /// Path to the generated private key file.
    pub key_path: PathBuf,
    /// Final state of the setup process.
    pub state: KeySetupState,
    /// Error message if the setup failed.
    pub error_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_hostname() {
        assert_eq!(sanitize_hostname("web-prod-1"), "web-prod-1");
        assert_eq!(sanitize_hostname("my server (prod)"), "my_server__prod_");
        assert_eq!(sanitize_hostname("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_hostname(""), "unnamed_host");
        assert_eq!(sanitize_hostname("a/b\\c:d*e?f"), "a_b_c_d_e_f");

        // Test truncation.
        let long_name = "a".repeat(100);
        assert_eq!(sanitize_hostname(&long_name).len(), MAX_HOSTNAME_LENGTH);
    }

    #[tokio::test]
    #[ignore] // Run manually with: cargo test test_key_generation_and_conversion -- --ignored
    async fn test_key_generation_and_conversion() {
        // Generate a test key pair
        let test_host = "test_conversion_host";
        let result = generate_key_pair(test_host, KeyType::Ed25519).await;

        assert!(result.is_ok(), "Key generation should succeed");
        let (private_key_path, public_key_path) = result.unwrap();

        // Check that files exist
        assert!(private_key_path.exists(), "Private key file should exist");
        assert!(public_key_path.exists(), "Public key file should exist");

        // Read the private key and check format
        let private_key_content = tokio::fs::read_to_string(&private_key_path).await.unwrap();
        let first_line = private_key_content.lines().next().unwrap();

        // After conversion, it should be OpenSSH format, not PKCS#8
        assert!(
            first_line.contains("BEGIN OPENSSH PRIVATE KEY")
                || first_line.contains("BEGIN PRIVATE KEY"),
            "Key should be in OpenSSH or PKCS#8 format, got: {}",
            first_line
        );

        // Try to extract public key using ssh-keygen (validates the key is readable by OpenSSH)
        let output = tokio::process::Command::new("ssh-keygen")
            .args(&["-y", "-f"])
            .arg(&private_key_path)
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "ssh-keygen should be able to read the generated key. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Cleanup
        let _ = tokio::fs::remove_file(&private_key_path).await;
        let _ = tokio::fs::remove_file(&public_key_path).await;
    }

    #[test]
    fn test_key_setup_step_ordering() {
        let steps = KeySetupStep::all_steps();
        assert_eq!(steps[0], KeySetupStep::GenerateKey);
        assert_eq!(steps[5], KeySetupStep::FinalCheck);
        assert_eq!(steps.len(), 6);
    }

    #[test]
    fn test_key_setup_machine_verify_failure_stops_process() {
        let mut machine = KeySetupMachine::new();
        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(
            KeySetupStep::VerifyKeyAuth,
            Err(anyhow!("connection refused")),
        );

        // Password must NOT be disabled.
        assert_eq!(machine.state(), &KeySetupState::FailedSafe);
        assert!(!machine.password_disabled());
    }

    #[test]
    fn test_key_setup_machine_no_sudo_partial_success() {
        let mut machine = KeySetupMachine::new();
        machine.set_has_sudo(false);

        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));

        // Key works but no sudo → PartialSuccess.
        assert_eq!(machine.state(), &KeySetupState::PartialSuccess);
        assert!(!machine.password_disabled());
    }

    #[test]
    fn test_key_setup_machine_final_check_failure_triggers_rollback() {
        let mut machine = KeySetupMachine::new();
        machine.step_result(KeySetupStep::GenerateKey, Ok(()));
        machine.step_result(KeySetupStep::CopyPublicKey, Ok(()));
        machine.step_result(KeySetupStep::VerifyKeyAuth, Ok(()));
        machine.step_result(KeySetupStep::DisablePassword, Ok(()));
        machine.step_result(KeySetupStep::ReloadSshd, Ok(()));
        machine.step_result(KeySetupStep::FinalCheck, Err(anyhow!("timeout")));

        // Password is disabled but key doesn't work → rollback needed.
        assert_eq!(machine.state(), &KeySetupState::NeedsRollback);
        assert!(machine.password_disabled());
    }

    #[test]
    fn test_authorized_keys_command_escapes_quotes() {
        let pubkey = "ssh-ed25519 AAAA... user's key";
        let cmd = build_authorized_keys_command(pubkey);

        // Should escape single quotes.
        assert!(cmd.contains("user'\\''s"));
        // Should use >> (append, not overwrite).
        assert!(cmd.contains(">> ~/.ssh/authorized_keys"));
        // Should NOT use > (overwrite).
        assert!(!cmd.contains(" > ~/.ssh/authorized_keys"));
    }

    #[test]
    fn test_disable_password_command_creates_backup() {
        let cmd = build_disable_password_command();

        // Should create timestamped backup.
        assert!(cmd.contains("omnyssh_backup."));
        // Should run sshd -t for validation.
        assert!(cmd.contains("sshd -t"));
        // Should comment out Include directives to prevent overrides.
        assert!(cmd.contains("'s/^Include\\s/#Include /'"));
        // Should disable all password auth methods.
        assert!(cmd.contains("PasswordAuthentication no"));
        assert!(cmd.contains("ChallengeResponseAuthentication no"));
        assert!(cmd.contains("KbdInteractiveAuthentication no"));
        // Should disable PAM to prevent bypassing password auth.
        assert!(cmd.contains("UsePAM no"));
    }

    #[test]
    fn test_reload_sshd_uses_reload_not_restart() {
        let cmd = build_reload_sshd_command();

        // Should use reload, not restart.
        assert!(cmd.contains("reload"));
        assert!(!cmd.contains("restart"));
    }
}
