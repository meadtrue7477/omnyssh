use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::utils::platform;

/// Scope of a command snippet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SnippetScope {
    /// Available for any host.
    Global,
    /// Available only for a specific host (identified by name).
    Host,
}

/// A saved command snippet stored in `~/.config/omnyssh/snippets.toml`.
/// Mirrors the format defined in §7 of the technical specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub name: String,
    pub command: String,
    pub scope: SnippetScope,
    /// Required when `scope == Host`.
    pub host: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Named placeholder parameters, e.g. `["service_name"]`.
    pub params: Option<Vec<String>>,
}

/// Root container that maps to the TOML array-of-tables format.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SnippetsFile {
    #[serde(default)]
    pub snippets: Vec<Snippet>,
}

/// Loads snippets from `~/.config/omnyssh/snippets.toml`.
///
/// Returns an empty `Vec` if the file does not exist yet.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or parsed.
pub fn load_snippets() -> anyhow::Result<Vec<Snippet>> {
    let path = platform::snippets_config_path().context("Cannot determine snippets config path")?;

    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;

    let file: SnippetsFile =
        toml::from_str(&content).with_context(|| format!("Failed to parse {}", path.display()))?;

    Ok(file.snippets)
}

/// Persists snippets to `~/.config/omnyssh/snippets.toml`.
///
/// # Errors
/// Returns an error if the directory cannot be created or the file cannot
/// be written.
pub fn save_snippets(snippets: &[Snippet]) -> anyhow::Result<()> {
    let dir = platform::app_config_dir().context("Cannot determine app config directory")?;

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create directory {}", dir.display()))?;

    let path = dir.join("snippets.toml");

    let file = SnippetsFile {
        snippets: snippets.to_vec(),
    };
    let content = toml::to_string_pretty(&file).context("Failed to serialise snippets")?;

    // Write to a temp file and rename for atomic replacement (avoids a corrupt
    // snippets.toml if the process is interrupted mid-write).
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, &content)
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
