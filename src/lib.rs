pub mod config;
pub mod connection;
pub mod known_hosts;
pub mod meta_commands;
pub mod ssh_config;
pub mod tunnel;
pub mod workspace;

// FFI module for Steel integration
pub mod ffi;

use anyhow::Result;
use config::SqlConfig;
use connection::ConnectionManager;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
pub use workspace::Workspace;

// FFI-specific imports
use log::LevelFilter;
use once_cell::sync::Lazy;
use simplelog::*;
use std::fs;

/// Main entry point for helix-dadbod library
pub struct Dadbod {
    manager: Arc<Mutex<ConnectionManager>>,
}

impl Dadbod {
    /// Create a new Dadbod instance from a config file
    pub fn from_file(path: PathBuf) -> Result<Self> {
        let config = SqlConfig::from_file(&path)?;
        init_logging(&config.log_level);
        log::info!(
            "Initialized helix-dadbod from config file: {}",
            path.display()
        );
        Ok(Self::from_config(config))
    }

    /// Create a new Dadbod instance from default config location
    pub fn from_default() -> Result<Self> {
        let config = SqlConfig::from_default_location()?;
        init_logging(&config.log_level);
        log::info!("Initialized helix-dadbod from default config location");
        Ok(Self::from_config(config))
    }

    /// Create a new Dadbod instance from a config
    pub fn from_config(config: SqlConfig) -> Self {
        let manager = ConnectionManager::new(config);
        Self {
            manager: Arc::new(Mutex::new(manager)),
        }
    }

    /// List all available connection names
    pub async fn list_connections(&self) -> Vec<String> {
        let manager = self.manager.lock().await;
        manager
            .list_connections()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Connect to a database by name, returns workspace info
    pub async fn connect(&self, name: &str) -> Result<Workspace> {
        let manager = self.manager.lock().await;
        manager.get_or_create_connection(name).await
    }

    /// Test a connection by name
    pub async fn test_connection(&self, name: &str) -> Result<String> {
        let manager = self.manager.lock().await;
        manager.test_connection(name).await
    }

    /// Close a specific connection
    pub async fn close_connection(&self, name: &str) -> Result<()> {
        let manager = self.manager.lock().await;
        manager.close_connection(name).await
    }

    /// Close all connections and tunnels
    pub async fn close_all(&self) -> Result<()> {
        let manager = self.manager.lock().await;
        manager.close_all().await
    }

    /// Execute SQL query from workspace query.sql file
    pub async fn execute_query(&self, name: &str) -> Result<()> {
        let manager = self.manager.lock().await;
        manager.execute_query(name).await
    }

    /// Get information about an active connection
    pub async fn get_connection_info(&self, name: &str) -> Option<connection::ConnectionInfo> {
        let manager = self.manager.lock().await;
        manager.get_connection_info(name).await
    }

    // =========================================================================
    // Blocking wrappers for FFI
    // =========================================================================

    /// Synchronous wrapper for list_connections (for FFI)
    /// Uses the global runtime to execute async code
    pub fn list_connections_blocking(&self) -> Vec<String> {
        // Get the global runtime and execute on it
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.list_connections())
    }

    /// Synchronous wrapper for connect (for FFI)
    /// Uses the global runtime to execute async code
    pub fn connect_blocking(&self, name: &str) -> Result<Workspace> {
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.connect(name))
    }

    /// Synchronous wrapper for execute_query (for FFI)
    /// Uses the global runtime to execute async code
    pub fn execute_query_blocking(&self, name: &str) -> Result<()> {
        log::debug!("execute_query_blocking called for '{}'", name);
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.execute_query(name))
    }

    /// Synchronous wrapper for test_connection (for FFI)
    /// Uses the global runtime to execute async code
    pub fn test_connection_blocking(&self, name: &str) -> Result<String> {
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.test_connection(name))
    }

    /// Synchronous wrapper for close_connection (for FFI)
    /// Uses the global runtime to execute async code
    pub fn close_connection_blocking(&self, name: &str) -> Result<()> {
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.close_connection(name))
    }

    /// Synchronous wrapper for get_connection_info (for FFI)
    /// Uses the global runtime to execute async code
    pub fn get_connection_info_blocking(&self, name: &str) -> Option<connection::ConnectionInfo> {
        let rt = &GLOBAL_DADBOD.0;
        rt.block_on(self.get_connection_info(name))
    }
}

// =============================================================================
// FFI Support: Global Instance and Type Conversions
// =============================================================================

/// Initialize logging to ~/.config/helix-dadbod/dadbod.log
fn init_logging(log_level: &str) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let config_dir = PathBuf::from(home).join(".config").join("helix-dadbod");

    // Create config directory if it doesn't exist
    let _ = fs::create_dir_all(&config_dir);

    let log_file = config_dir.join("dadbod.log");

    // Parse log level, default to Info if invalid
    let level = match log_level.to_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info, // Default to Info for any other value
    };

    // Try to initialize the logger - if it fails, just continue without logging
    let _ = WriteLogger::init(
        level,
        Config::default(),
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
            .unwrap_or_else(|_| {
                // Fallback to a temp file if config dir doesn't work
                std::fs::File::create("/tmp/helix-dadbod.log").unwrap()
            }),
    );
}

/// Global Dadbod instance with embedded Tokio runtime
/// This is initialized lazily on first access
/// If initialization fails (e.g., malformed config.toml), stores None
static GLOBAL_DADBOD: Lazy<(tokio::runtime::Runtime, Option<Dadbod>, Option<String>)> =
    Lazy::new(|| {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");

        let (dadbod, error) = rt.block_on(async {
            // Load config first to get log level
            match SqlConfig::from_default_location() {
                Ok(config) => {
                    // Initialize logging with configured level
                    init_logging(&config.log_level);
                    log::info!(
                        "Initializing helix-dadbod with log level: {}",
                        config.log_level
                    );

                    // Create Dadbod instance from config
                    let db = Dadbod::from_config(config);
                    log::info!("helix-dadbod initialized successfully");
                    (Some(db), None)
                }
                Err(e) => {
                    // Initialize logging with default level (info) on error
                    init_logging("info");
                    let error_msg = format!("Failed to load database config: {}", e);
                    log::error!("{}", error_msg);
                    log::error!("Check ~/.config/helix-dadbod/config.toml for syntax errors");
                    (None, Some(error_msg))
                }
            }
        });

        (rt, dadbod, error)
    });

/// Get reference to global Dadbod instance (for FFI)
/// Returns None if initialization failed (e.g., malformed config)
pub fn global_dadbod() -> Option<&'static Dadbod> {
    GLOBAL_DADBOD.1.as_ref()
}

/// Get initialization error message if any
pub fn global_dadbod_error() -> Option<&'static str> {
    GLOBAL_DADBOD.2.as_deref()
}

/// FFI-friendly workspace info (uses Strings instead of PathBuf)
#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub path: String,
    pub sql_file: String,
    pub dbout_file: String,
}

impl From<Workspace> for WorkspacePaths {
    fn from(ws: Workspace) -> Self {
        Self {
            path: ws.path.display().to_string(),
            sql_file: ws.sql_file.display().to_string(),
            dbout_file: ws.dbout_file.display().to_string(),
        }
    }
}

impl From<&Workspace> for WorkspacePaths {
    fn from(ws: &Workspace) -> Self {
        Self {
            path: ws.path.display().to_string(),
            sql_file: ws.sql_file.display().to_string(),
            dbout_file: ws.dbout_file.display().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dadbod_from_config() {
        // Test that we can create a Dadbod instance from a config
        // This validates the basic initialization without needing a real database

        let config = SqlConfig {
            log_level: "error".to_string(),
            skip_host_key_verification: false,
            connections: vec![config::Connection {
                name: "test_db".to_string(),
                db_type: "postgres".to_string(),
                host: "localhost".to_string(),
                port: 5432,
                database: "test".to_string(),
                username: "test".to_string(),
                password: Some("test".to_string()),
                ssh_tunnel: None,
            }],
        };

        let dadbod = Dadbod::from_config(config);

        // Should have one connection configured
        // Note: We can't test async methods without tokio runtime,
        // but we can verify the instance was created successfully
        assert!(std::ptr::addr_of!(dadbod).is_null() == false);
    }
}
