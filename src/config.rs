use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SqlConfig {
    #[serde(default)]
    pub connections: Vec<Connection>,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Skip SSH host key verification (INSECURE - only for testing/dev environments)
    #[serde(default)]
    pub skip_host_key_verification: bool,
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Connection {
    pub name: String,
    #[serde(rename = "type")]
    pub db_type: String,
    pub host: String,
    #[serde(default = "default_postgres_port")]
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
    pub ssh_tunnel: Option<SshTunnel>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SshTunnel {
    /// Explicit SSH configuration
    Explicit {
        host: String,
        #[serde(default = "default_ssh_port")]
        port: u16,
        user: String,
        /// Optional private key path, defaults to ~/.ssh/id_rsa or ~/.ssh/id_ed25519
        key_path: Option<PathBuf>,
    },
    /// Reference to SSH config entry
    ConfigRef { ssh_config: String },
}

fn default_postgres_port() -> u16 {
    5432
}

fn default_ssh_port() -> u16 {
    22
}

impl SqlConfig {
    /// Load configuration from a TOML file
    pub fn from_file(path: &PathBuf) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: SqlConfig = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Load from default location (./config.toml or ~/.config/helix-dadbod/config.toml)
    pub fn from_default_location() -> Result<Self> {
        // Try current directory first
        let local_path = PathBuf::from("config.toml");
        if local_path.exists() {
            return Self::from_file(&local_path);
        }

        // Try Unix-style ~/.config/helix-dadbod/config.toml
        if let Some(home) = dirs::home_dir() {
            let unix_config = home
                .join(".config")
                .join("helix-dadbod")
                .join("config.toml");
            if unix_config.exists() {
                return Self::from_file(&unix_config);
            }
        }

        anyhow::bail!(
            "No config.toml found in:\n  \
             - ./config.toml\n  \
             - ~/.config/helix-dadbod/config.toml"
        )
    }

    /// Get connection by name
    pub fn get_connection(&self, name: &str) -> Option<&Connection> {
        self.connections.iter().find(|c| c.name == name)
    }

    /// List all connection names
    pub fn list_connections(&self) -> Vec<&str> {
        self.connections.iter().map(|c| c.name.as_str()).collect()
    }
}

impl Connection {
    /// Check if this connection requires an SSH tunnel
    pub fn needs_tunnel(&self) -> bool {
        self.ssh_tunnel.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_explicit_ssh() {
        let toml = r#"
            [[connections]]
            name = "test"
            type = "postgres"
            host = "localhost"
            database = "mydb"
            username = "user"

            [connections.ssh_tunnel]
            host = "jump.example.com"
            port = 22
            user = "sshuser"
        "#;

        let config: SqlConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.connections.len(), 1);
        assert!(config.connections[0].needs_tunnel());
    }

    #[test]
    fn test_parse_ssh_config_ref() {
        let toml = r#"
            [[connections]]
            name = "test"
            type = "postgres"
            host = "localhost"
            database = "mydb"
            username = "user"

            [connections.ssh_tunnel]
            ssh_config = "production-server"
        "#;

        let config: SqlConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.connections.len(), 1);
        assert!(config.connections[0].needs_tunnel());
    }

    #[test]
    fn test_skip_host_key_verification_defaults_to_false() {
        let toml = r#"
            [[connections]]
            name = "test"
            type = "postgres"
            host = "localhost"
            database = "mydb"
            username = "user"
        "#;

        let config: SqlConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.skip_host_key_verification, false);
    }

    #[test]
    fn test_skip_host_key_verification_can_be_enabled() {
        let toml = r#"
            skip_host_key_verification = true

            [[connections]]
            name = "test"
            type = "postgres"
            host = "localhost"
            database = "mydb"
            username = "user"
        "#;

        let config: SqlConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.skip_host_key_verification, true);
    }
}
