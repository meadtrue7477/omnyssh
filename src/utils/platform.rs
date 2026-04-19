use std::path::PathBuf;

/// Returns the path to the user's SSH config file.
///
/// - Linux / macOS: `~/.ssh/config`
/// - Windows:       `%USERPROFILE%\.ssh\config`
///
/// Uses the `dirs` crate so we never hardcode `~`.
pub fn ssh_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("config"))
}

/// Returns the application config directory.
///
/// - Linux / macOS: `~/.config/omnyssh/`
/// - Windows:       `%APPDATA%\omnyssh\`
pub fn app_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("omnyssh"))
}

/// Returns the path to the main application config file.
pub fn app_config_path() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("config.toml"))
}

/// Returns the path to the hosts config file.
pub fn hosts_config_path() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("hosts.toml"))
}

/// Returns the path to the snippets config file.
pub fn snippets_config_path() -> Option<PathBuf> {
    app_config_dir().map(|d| d.join("snippets.toml"))
}
