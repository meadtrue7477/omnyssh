use clap::Parser;

/// OmnySSH — TUI SSH dashboard & server manager.
///
/// Manage all your SSH servers from a single terminal window.
/// Dashboard with live metrics, SFTP file manager, command snippets,
/// and multi-session tabs with split-view.
#[derive(Parser, Debug)]
#[command(
    name = "omny",
    version,
    about = "TUI SSH dashboard & server manager",
    long_about = None,
)]
pub struct Cli {
    /// Path to a custom config file.
    ///
    /// Defaults to ~/.config/omnyssh/config.toml (Linux/macOS)
    /// or %APPDATA%\\omnyssh\\config.toml (Windows).
    #[arg(short, long, value_name = "FILE")]
    pub config: Option<std::path::PathBuf>,

    /// Override the color theme (default | dracula | nord | gruvbox).
    ///
    /// This overrides the theme setting from the config file.
    #[arg(short, long, value_name = "THEME")]
    pub theme: Option<String>,

    /// Enable verbose debug logging (written to stderr).
    #[arg(short, long)]
    pub verbose: bool,
}
