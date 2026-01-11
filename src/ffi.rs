use crate::{global_dadbod, global_dadbod_error, WorkspacePaths};
use std::panic;
use steel::{
    declare_module,
    rvals::Custom,
    steel_vm::ffi::{FFIModule, RegisterFFIFn},
};

/// FFI-friendly wrapper for workspace info that implements Steel's Custom trait
#[derive(Clone, Debug)]
pub struct SteelWorkspaceInfo {
    pub path: String,
    pub sql_file: String,
    pub dbout_file: String,
}

impl Custom for SteelWorkspaceInfo {}

impl From<WorkspacePaths> for SteelWorkspaceInfo {
    fn from(wp: WorkspacePaths) -> Self {
        Self {
            path: wp.path,
            sql_file: wp.sql_file,
            dbout_file: wp.dbout_file,
        }
    }
}

// Add getters so Steel can access fields
impl SteelWorkspaceInfo {
    pub fn path(&self) -> String {
        self.path.clone()
    }

    pub fn sql_file(&self) -> String {
        self.sql_file.clone()
    }

    pub fn dbout_file(&self) -> String {
        self.dbout_file.clone()
    }
}

/// List all available database connections from config.toml
fn list_connections_ffi() -> Vec<String> {
    match global_dadbod() {
        Some(dadbod) => dadbod.list_connections_blocking(),
        None => {
            log::error!("Cannot list connections: helix-dadbod not initialized");
            Vec::new()
        }
    }
}

/// Connect to a database by name, returns workspace info
/// Returns None on error (logs error instead of panicking)
fn connect_ffi(name: &str) -> Option<SteelWorkspaceInfo> {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match global_dadbod() {
        Some(dadbod) => match dadbod.connect_blocking(name) {
            Ok(workspace) => {
                let workspace_paths: WorkspacePaths = workspace.into();
                Some(workspace_paths.into())
            }
            Err(e) => {
                log::error!("Failed to connect to '{}': {}", name, e);
                None
            }
        },
        None => {
            log::error!("Cannot connect: helix-dadbod not initialized (check config.toml)");
            None
        }
    }));

    match result {
        Ok(value) => value,
        Err(_) => {
            log::error!("Panic occurred while connecting to '{}'", name);
            None
        }
    }
}

/// Test a database connection, returns database version string
/// Returns empty string on error (logs error instead of panicking)
fn test_connection_ffi(name: &str) -> String {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match global_dadbod() {
        Some(dadbod) => match dadbod.test_connection_blocking(name) {
            Ok(version) => version,
            Err(e) => {
                log::error!("Connection test failed for '{}': {}", name, e);
                String::new()
            }
        },
        None => {
            log::error!("Cannot test connection: helix-dadbod not initialized (check config.toml)");
            String::new()
        }
    }));

    match result {
        Ok(value) => value,
        Err(_) => {
            log::error!("Panic occurred while testing connection '{}'", name);
            String::new()
        }
    }
}

/// Execute SQL query from workspace query.sql file
/// Returns error message on failure (logs error instead of panicking)
fn execute_query_ffi(name: &str) -> String {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match global_dadbod() {
        Some(dadbod) => match dadbod.execute_query_blocking(name) {
            Ok(_) => "Query executed successfully".to_string(),
            Err(e) => {
                log::error!("Query execution failed for '{}': {}", name, e);
                format!("Error: {}", e)
            }
        },
        None => {
            log::error!("Cannot execute query: helix-dadbod not initialized (check config.toml)");
            "Error: Database not initialized - check config.toml".to_string()
        }
    }));

    match result {
        Ok(value) => value,
        Err(_) => {
            log::error!("Panic occurred while executing query for '{}'", name);
            "Error: Panic occurred during query execution".to_string()
        }
    }
}

/// Close a specific database connection and its SSH tunnel
/// Returns error message on failure (logs error instead of panicking)
fn close_connection_ffi(name: &str) -> String {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match global_dadbod() {
        Some(dadbod) => match dadbod.close_connection_blocking(name) {
            Ok(_) => format!("Connection '{}' closed successfully", name),
            Err(e) => {
                log::error!("Failed to close connection '{}': {}", name, e);
                format!("Error: {}", e)
            }
        },
        None => {
            log::error!(
                "Cannot close connection: helix-dadbod not initialized (check config.toml)"
            );
            "Error: Database not initialized - check config.toml".to_string()
        }
    }));

    match result {
        Ok(value) => value,
        Err(_) => {
            log::error!("Panic occurred while closing connection '{}'", name);
            "Error: Panic occurred while closing connection".to_string()
        }
    }
}

/// Get workspace directory path for a connection
/// Returns empty string if connection is not active (logs error instead of panicking)
fn get_workspace_path_ffi(name: &str) -> String {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| match global_dadbod() {
        Some(dadbod) => match dadbod.get_connection_info_blocking(name) {
            Some(info) => info.workspace.path.display().to_string(),
            None => {
                log::error!("Connection '{}' is not active", name);
                String::new()
            }
        },
        None => {
            log::error!(
                "Cannot get workspace path: helix-dadbod not initialized (check config.toml)"
            );
            String::new()
        }
    }));

    match result {
        Ok(value) => value,
        Err(_) => {
            log::error!("Panic occurred while getting workspace path for '{}'", name);
            String::new()
        }
    }
}

/// Check if helix-dadbod initialized successfully
/// Returns error message if initialization failed, empty string if successful
fn get_init_error_ffi() -> String {
    global_dadbod_error()
        .map(|e| e.to_string())
        .unwrap_or_default()
}

declare_module!(create_module);

fn create_module() -> FFIModule {
    let mut module = FFIModule::new("steel/helix-dadbod");

    module
        .register_fn("Dadbod::list_connections", list_connections_ffi)
        .register_fn("Dadbod::connect", connect_ffi)
        .register_fn("Dadbod::test_connection", test_connection_ffi)
        .register_fn("Dadbod::execute_query", execute_query_ffi)
        .register_fn("Dadbod::close_connection", close_connection_ffi)
        .register_fn("Dadbod::get_workspace_path", get_workspace_path_ffi)
        .register_fn("Dadbod::get_init_error", get_init_error_ffi)
        // Register workspace info getters
        .register_fn("WorkspaceInfo-path", SteelWorkspaceInfo::path)
        .register_fn("WorkspaceInfo-sql_file", SteelWorkspaceInfo::sql_file)
        .register_fn("WorkspaceInfo-dbout_file", SteelWorkspaceInfo::dbout_file);

    module
}
