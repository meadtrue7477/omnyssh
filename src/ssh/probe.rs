//! Probe script generation and output parsing for service discovery.
//!
//! The Quick Scan probe is a single bash script that collects maximum
//! information in one SSH invocation. Output is delimited by
//! section markers (`===OMNYSSH:SECTION===`) for easy parsing.

use std::collections::HashMap;

/// Generates the Quick Scan probe bash script.
///
/// This script runs multiple commands and delimits their output with
/// section markers for structured parsing. All commands use stderr
/// redirection to /dev/null for graceful failure.
pub fn generate_quick_scan_script() -> &'static str {
    r#"cat << 'OMNYSSH_PROBE_EOF' | bash
echo "===OMNYSSH:OS==="
cat /etc/os-release 2>/dev/null | head -5
echo "===OMNYSSH:SERVICES==="
systemctl list-units --type=service --state=running --no-pager --no-legend 2>/dev/null | awk '{print $1}' | head -50
echo "===OMNYSSH:DOCKER==="
docker ps --format '{{.ID}}\t{{.Names}}\t{{.Status}}\t{{.Image}}' 2>/dev/null | head -30
echo "===OMNYSSH:LISTEN==="
ss -tlnp 2>/dev/null | tail -n +2 | head -30
echo "===OMNYSSH:PROCESS==="
ps aux --sort=-%mem 2>/dev/null | head -15
OMNYSSH_PROBE_EOF
"#
}

/// Parsed output from the probe script, organized by section.
#[derive(Debug, Clone, Default)]
pub struct ProbeOutput {
    sections: HashMap<String, String>,
}

impl ProbeOutput {
    /// Parse the probe script output into sections.
    ///
    /// Each section is delimited by `===OMNYSSH:NAME===` markers.
    /// Returns `Ok` even for empty or malformed input (graceful degradation).
    ///
    /// # Errors
    /// Never returns an error — unknown or malformed output is silently ignored.
    pub fn parse(output: &str) -> anyhow::Result<Self> {
        let mut sections = HashMap::new();
        let mut current_section: Option<String> = None;
        let mut current_content = String::new();

        for line in output.lines() {
            let trimmed = line.trim();

            // Check if this is a section marker
            if trimmed.starts_with("===OMNYSSH:") && trimmed.ends_with("===") {
                // Save previous section if it exists
                if let Some(section_name) = current_section.take() {
                    sections.insert(section_name, current_content.trim().to_string());
                    current_content.clear();
                }

                // Extract new section name (remove markers)
                let section_name = trimmed
                    .strip_prefix("===OMNYSSH:")
                    .and_then(|s| s.strip_suffix("==="))
                    .unwrap_or("")
                    .to_string();

                current_section = Some(section_name);
            } else if current_section.is_some() {
                // Accumulate content for current section
                current_content.push_str(line);
                current_content.push('\n');
            }
        }

        // Don't forget the last section
        if let Some(section_name) = current_section {
            sections.insert(section_name, current_content.trim().to_string());
        }

        Ok(Self { sections })
    }

    /// Check if a specific section exists and has non-empty content.
    pub fn has_section(&self, name: &str) -> bool {
        self.sections
            .get(name)
            .map(|content| !content.is_empty())
            .unwrap_or(false)
    }

    /// Get the content of a section, or None if it doesn't exist.
    pub fn get_section(&self, name: &str) -> Option<&str> {
        self.sections.get(name).map(|s| s.as_str())
    }

    /// Get all section names that exist in the output.
    pub fn section_names(&self) -> Vec<&str> {
        self.sections.keys().map(|s| s.as_str()).collect()
    }

    /// Parse OS information from the OS section.
    ///
    /// Extracts OS name and version from /etc/os-release format.
    /// Returns a formatted string like "Ubuntu 22.04 LTS" or "Debian GNU/Linux 11".
    pub fn parse_os_info(&self) -> Option<String> {
        let os_section = self.get_section("OS")?;

        let mut name: Option<String> = None;
        let mut version: Option<String> = None;
        let mut pretty_name: Option<String> = None;

        for line in os_section.lines() {
            let line = line.trim();

            // Try to extract PRETTY_NAME first (most user-friendly)
            if line.starts_with("PRETTY_NAME=") {
                pretty_name = extract_value(line, "PRETTY_NAME=");
            } else if line.starts_with("NAME=") {
                name = extract_value(line, "NAME=");
            } else if line.starts_with("VERSION=") {
                version = extract_value(line, "VERSION=");
            }
        }

        // Prefer PRETTY_NAME if available
        if let Some(pretty) = pretty_name {
            return Some(pretty);
        }

        // Otherwise combine NAME and VERSION
        match (name, version) {
            (Some(n), Some(v)) => Some(format!("{} {}", n, v)),
            (Some(n), None) => Some(n),
            _ => None,
        }
    }
}

/// Helper function to extract value from os-release format line.
/// Handles both quoted ("value") and unquoted (value) formats.
fn extract_value(line: &str, prefix: &str) -> Option<String> {
    let value = line.strip_prefix(prefix)?.trim();

    // Remove surrounding quotes if present (both " and ' work the same way)
    if (value.starts_with('"') && value.ends_with('"')
        || value.starts_with('\'') && value.ends_with('\''))
        && value.len() >= 2
    {
        Some(value[1..value.len() - 1].to_string())
    } else {
        Some(value.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_probe_empty_output() {
        let result = ProbeOutput::parse("").expect("should parse empty");
        assert_eq!(result.section_names().len(), 0);
    }

    #[test]
    fn test_parse_probe_garbage_output() {
        let result = ProbeOutput::parse("random\ngarbage\ntext").expect("should parse garbage");
        assert_eq!(result.section_names().len(), 0);
    }

    #[test]
    fn test_parse_probe_single_section() {
        let output = "===OMNYSSH:OS===\nUbuntu 22.04 LTS\n===OMNYSSH:SERVICES===\nsshd.service\n";
        let result = ProbeOutput::parse(output).expect("should parse");
        assert!(result.has_section("OS"));
        assert!(result.has_section("SERVICES"));
        assert_eq!(result.get_section("OS"), Some("Ubuntu 22.04 LTS"));
        assert_eq!(result.get_section("SERVICES"), Some("sshd.service"));
    }

    #[test]
    fn test_parse_probe_multiple_sections() {
        let output = r#"===OMNYSSH:OS===
NAME="Ubuntu"
VERSION="22.04 LTS"
===OMNYSSH:DOCKER===
abc123	nginx-proxy	Up 2 hours	nginx:latest
def456	db-master	Up 5 days	postgres:15
===OMNYSSH:LISTEN===
0.0.0.0:22	LISTEN
0.0.0.0:80	LISTEN
"#;
        let result = ProbeOutput::parse(output).expect("should parse");
        assert!(result.has_section("OS"));
        assert!(result.has_section("DOCKER"));
        assert!(result.has_section("LISTEN"));

        let docker_section = result
            .get_section("DOCKER")
            .expect("docker section should exist");
        assert!(docker_section.contains("nginx-proxy"));
        assert!(docker_section.contains("postgres:15"));
    }

    #[test]
    fn test_parse_probe_with_empty_sections() {
        let output = "===OMNYSSH:OS===\nUbuntu\n===OMNYSSH:DOCKER===\n===OMNYSSH:SERVICES===\nsshd.service\n";
        let result = ProbeOutput::parse(output).expect("should parse");
        assert!(result.has_section("OS"));
        assert!(!result.has_section("DOCKER")); // Empty section = false
        assert!(result.has_section("SERVICES"));
    }

    #[test]
    fn test_generate_quick_scan_script() {
        let script = generate_quick_scan_script();
        assert!(script.contains("===OMNYSSH:OS==="));
        assert!(script.contains("===OMNYSSH:SERVICES==="));
        assert!(script.contains("===OMNYSSH:DOCKER==="));
        assert!(script.contains("===OMNYSSH:LISTEN==="));
        assert!(script.contains("===OMNYSSH:PROCESS==="));
        assert!(script.contains("/etc/os-release"));
        assert!(script.contains("systemctl list-units"));
        assert!(script.contains("docker ps"));
    }

    #[test]
    fn test_section_names() {
        let output = "===OMNYSSH:OS===\ndata\n===OMNYSSH:DOCKER===\nmore data\n";
        let result = ProbeOutput::parse(output).expect("should parse");
        let names = result.section_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"OS"));
        assert!(names.contains(&"DOCKER"));
    }
}
