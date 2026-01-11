//! SSH config file parser
//!
//! Parses ~/.ssh/config files to extract connection details for SSH tunnels

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// Parsed SSH configuration for a host
#[derive(Debug, Clone)]
pub struct SshHostConfig {
    pub hostname: String,
    pub port: u16,
    pub user: Option<String>,
    pub identity_file: Option<PathBuf>,
}

/// Parse SSH config file and extract configuration for a specific host
pub fn parse_ssh_config(host_name: &str) -> Result<SshHostConfig> {
    let config_path = get_ssh_config_path()?;

    let contents = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read SSH config from {}", config_path.display()))?;

    parse_host_from_config(&contents, host_name).with_context(|| {
        format!(
            "Host '{}' not found in {}",
            host_name,
            config_path.display()
        )
    })
}

/// Get the path to the SSH config file
fn get_ssh_config_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".ssh").join("config"))
}

/// Parse SSH config content and extract configuration for a specific host
fn parse_host_from_config(content: &str, target_host: &str) -> Result<SshHostConfig> {
    let mut current_host: Option<String> = None;
    let mut host_config: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip comments and empty lines
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Split into key and value
        let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
        if parts.len() < 2 {
            continue;
        }

        let key = parts[0];
        let value = parts[1].trim();

        match key {
            "Host" => {
                // If we were parsing the target host and now found a new Host entry, we're done
                if current_host.as_deref() == Some(target_host) {
                    break;
                }

                // Start parsing a new host
                current_host = Some(value.to_string());
                host_config.clear();
            }
            _ => {
                // Only collect config for the target host
                if current_host.as_deref() == Some(target_host) {
                    host_config.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    // Check if we found the target host
    if current_host.as_deref() != Some(target_host) {
        anyhow::bail!("Host '{}' not found in SSH config", target_host);
    }

    // Extract required and optional fields
    let hostname = host_config
        .get("HostName")
        .or_else(|| host_config.get("Hostname"))
        .context("HostName not specified in SSH config")?
        .to_string();

    let port = host_config
        .get("Port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);

    let user = host_config.get("User").map(|u| u.to_string());

    let identity_file = host_config
        .get("IdentityFile")
        .map(|path| expand_tilde(path));

    Ok(SshHostConfig {
        hostname,
        port,
        user,
        identity_file,
    })
}

/// Expand ~ to the home directory
fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(stripped);
        }
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_host_from_config() {
        let config = r#"
# Comment line
Host example
    HostName example.com
    Port 2222
    User testuser
    IdentityFile ~/.ssh/example_key

Host another
    HostName another.com
    User anotheruser
"#;

        let result = parse_host_from_config(config, "example").unwrap();
        assert_eq!(result.hostname, "example.com");
        assert_eq!(result.port, 2222);
        assert_eq!(result.user.unwrap(), "testuser");
        assert!(result.identity_file.is_some());
    }

    #[test]
    fn test_parse_host_defaults() {
        let config = r#"
Host minimal
    HostName minimal.com
"#;

        let result = parse_host_from_config(config, "minimal").unwrap();
        assert_eq!(result.hostname, "minimal.com");
        assert_eq!(result.port, 22); // Default port
        assert!(result.user.is_none());
        assert!(result.identity_file.is_none());
    }

    #[test]
    fn test_parse_host_not_found() {
        let config = r#"
Host example
    HostName example.com
"#;

        let result = parse_host_from_config(config, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/test/path");
        assert!(!expanded.to_string_lossy().starts_with("~/"));

        let no_tilde = expand_tilde("/absolute/path");
        assert_eq!(no_tilde, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_parse_multiple_hosts() {
        let config = r#"
Host first
    HostName first.com
    Port 1111

Host second
    HostName second.com
    Port 2222

Host third
    HostName third.com
    Port 3333
"#;

        let result = parse_host_from_config(config, "second").unwrap();
        assert_eq!(result.hostname, "second.com");
        assert_eq!(result.port, 2222);
    }
}
