use crate::config::{Connection, SqlConfig};
use crate::meta_commands::MetaCommand;
use crate::tunnel::TunnelManager;
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use chrono::Local;
use comfy_table::{presets::UTF8_FULL, Table};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio_postgres::{types::Type, Client, NoTls};

/// Manages database connections
pub struct ConnectionManager {
    config: SqlConfig,
    tunnel_manager: TunnelManager,
    active_connections: Arc<Mutex<HashMap<String, ActiveConnection>>>,
}

/// An active database connection
pub struct ActiveConnection {
    pub client: Arc<Client>,
    pub connection_name: String,
    pub uses_tunnel: bool,
    pub local_port: Option<u16>,
    pub workspace: Workspace,
}

impl ConnectionManager {
    pub fn new(config: SqlConfig) -> Self {
        let skip_verification = config.skip_host_key_verification;
        Self {
            config,
            tunnel_manager: TunnelManager::new(skip_verification),
            active_connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// List all available connection names from config
    pub fn list_connections(&self) -> Vec<&str> {
        self.config.list_connections()
    }

    /// Get or create a connection by name, returns workspace info
    pub async fn get_or_create_connection(&self, name: &str) -> Result<Workspace> {
        log::info!("Attempting to connect to database: {}", name);
        let mut connections = self.active_connections.lock().await;

        // Check if connection already exists
        if let Some(active) = connections.get(name) {
            log::info!("Using existing connection to: {}", name);
            return Ok(active.workspace.clone());
        }

        // Get connection config
        let conn_config = self
            .config
            .get_connection(name)
            .with_context(|| format!("Connection '{}' not found in config", name))?;

        // Create new connection
        let active = self.create_connection(conn_config).await?;
        let workspace = active.workspace.clone();

        connections.insert(name.to_string(), active);

        log::info!("Successfully connected to: {}", name);
        Ok(workspace)
    }

    /// Create a new database connection
    async fn create_connection(&self, conn: &Connection) -> Result<ActiveConnection> {
        match conn.db_type.as_str() {
            "postgres" | "postgresql" => self.create_postgres_connection(conn).await,
            _ => anyhow::bail!("Unsupported database type: {}", conn.db_type),
        }
    }

    /// Create a PostgreSQL connection
    async fn create_postgres_connection(&self, conn: &Connection) -> Result<ActiveConnection> {
        let (host, port, uses_tunnel, local_port) = if let Some(ssh_config) = &conn.ssh_tunnel {
            // Connection requires SSH tunnel
            let local_port = self
                .tunnel_manager
                .get_or_create_tunnel(&conn.name, ssh_config, &conn.host, conn.port)
                .await
                .context("Failed to create SSH tunnel")?;

            ("localhost".to_string(), local_port, true, Some(local_port))
        } else {
            // Direct connection
            (conn.host.clone(), conn.port, false, None)
        };

        // Build connection string
        let mut conn_str = format!(
            "host={} port={} user={} dbname={}",
            host, port, conn.username, conn.database
        );

        if let Some(password) = &conn.password {
            conn_str.push_str(&format!(" password={}", password));
        }

        // Connect to database
        let (client, connection) = tokio_postgres::connect(&conn_str, NoTls)
            .await
            .with_context(|| format!("Failed to connect to database '{}'", conn.name))?;

        // Spawn the connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::error!("Connection error: {}", e);
            }
        });

        // Create workspace
        let workspace = Workspace::create(&conn.name)?;

        Ok(ActiveConnection {
            client: Arc::new(client),
            connection_name: conn.name.clone(),
            uses_tunnel,
            local_port,
            workspace,
        })
    }

    /// Close a specific connection
    pub async fn close_connection(&self, name: &str) -> Result<()> {
        let mut connections = self.active_connections.lock().await;

        if let Some(active) = connections.remove(name) {
            // Clean up workspace
            active.workspace.cleanup()?;

            // Close the database connection
            drop(active.client);

            // Close tunnel if it was used
            if active.uses_tunnel {
                self.tunnel_manager.close_tunnel(name).await?;
            }
        }

        Ok(())
    }

    /// Close all connections and tunnels
    pub async fn close_all(&self) -> Result<()> {
        let mut connections = self.active_connections.lock().await;

        for (_, active) in connections.drain() {
            // Clean up workspace
            let _ = active.workspace.cleanup();
            drop(active.client);
        }

        self.tunnel_manager.close_all().await?;

        Ok(())
    }

    /// Test a connection by name
    pub async fn test_connection(&self, name: &str) -> Result<String> {
        // Ensure connection exists
        self.get_or_create_connection(name).await?;

        // Get the client
        let connections = self.active_connections.lock().await;
        let active = connections
            .get(name)
            .context("Connection not found after creation")?;

        let row = active
            .client
            .query_one("SELECT version()", &[])
            .await
            .context("Failed to execute test query")?;

        let version: String = row.get(0);

        Ok(version)
    }

    /// Convert a PostgreSQL value to a string representation based on its type
    fn value_to_string(row: &tokio_postgres::Row, idx: usize, col_type: &Type) -> String {
        // Check type by name since Type doesn't implement PartialEq for constants
        if *col_type == Type::BOOL {
            return row
                .try_get::<_, Option<bool>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::INT2 {
            return row
                .try_get::<_, Option<i16>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::INT4 {
            return row
                .try_get::<_, Option<i32>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::INT8 {
            return row
                .try_get::<_, Option<i64>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::FLOAT4 {
            return row
                .try_get::<_, Option<f32>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::FLOAT8 {
            return row
                .try_get::<_, Option<f64>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::UUID {
            return row
                .try_get::<_, Option<uuid::Uuid>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::TIMESTAMP {
            return row
                .try_get::<_, Option<chrono::NaiveDateTime>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::TIMESTAMPTZ {
            return row
                .try_get::<_, Option<chrono::DateTime<chrono::Utc>>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::DATE {
            return row
                .try_get::<_, Option<chrono::NaiveDate>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::TIME {
            return row
                .try_get::<_, Option<chrono::NaiveTime>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::JSON || *col_type == Type::JSONB {
            return row
                .try_get::<_, Option<serde_json::Value>>(idx)
                .ok()
                .flatten()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "NULL".to_string());
        }

        if *col_type == Type::BYTEA {
            return row
                .try_get::<_, Option<Vec<u8>>>(idx)
                .ok()
                .flatten()
                .map(|v| format!("\\x{}", hex::encode(v)))
                .unwrap_or_else(|| "NULL".to_string());
        }

        // NUMERIC/DECIMAL types - handle as string to preserve precision
        if *col_type == Type::NUMERIC {
            return row
                .try_get::<_, Option<String>>(idx)
                .ok()
                .flatten()
                .unwrap_or_else(|| "NULL".to_string());
        }

        // Fallback: try as string for text types and all other types
        row.try_get::<_, Option<String>>(idx)
            .ok()
            .flatten()
            .unwrap_or_else(|| "NULL".to_string())
    }

    /// Strip SQL comments (both -- and /* */) from the input
    fn strip_sql_comments(sql: &str) -> String {
        let mut result = String::new();
        let mut chars = sql.chars().peekable();
        let mut in_multiline_comment = false;
        let mut in_single_line_comment = false;
        let mut current_line = String::new();

        while let Some(ch) = chars.next() {
            if in_multiline_comment {
                // Look for end of multiline comment */
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next(); // consume '/'
                    in_multiline_comment = false;
                }
                continue;
            }

            if in_single_line_comment {
                // Single-line comment ends at newline
                if ch == '\n' {
                    in_single_line_comment = false;
                    // Push the current line if it has content
                    let trimmed = current_line.trim();
                    if !trimmed.is_empty() {
                        if !result.is_empty() {
                            result.push('\n');
                        }
                        result.push_str(trimmed);
                    }
                    current_line.clear();
                }
                continue;
            }

            // Check for comment start
            if ch == '-' && chars.peek() == Some(&'-') {
                chars.next(); // consume second '-'
                in_single_line_comment = true;
                continue;
            }

            if ch == '/' && chars.peek() == Some(&'*') {
                chars.next(); // consume '*'
                in_multiline_comment = true;
                continue;
            }

            // Regular character
            if ch == '\n' {
                // Push the current line if it has content
                let trimmed = current_line.trim();
                if !trimmed.is_empty() {
                    if !result.is_empty() {
                        result.push('\n');
                    }
                    result.push_str(trimmed);
                }
                current_line.clear();
            } else {
                current_line.push(ch);
            }
        }

        // Don't forget the last line
        let trimmed = current_line.trim();
        if !trimmed.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(trimmed);
        }

        // Normalize multiple spaces to single space
        let mut normalized = String::new();
        let mut last_was_space = false;
        for ch in result.chars() {
            if ch == ' ' {
                if !last_was_space {
                    normalized.push(ch);
                    last_was_space = true;
                }
            } else {
                normalized.push(ch);
                last_was_space = false;
            }
        }

        normalized
    }

    /// Execute SQL query from workspace query.sql file
    pub async fn execute_query(&self, name: &str) -> Result<()> {
        let connections = self.active_connections.lock().await;
        let active = connections
            .get(name)
            .with_context(|| format!("Connection '{}' not active. Call connect() first.", name))?;

        // Read query from workspace
        let sql = active
            .workspace
            .read_query()
            .context("Failed to read query from query.sql")?;

        let sql = sql.trim();
        if sql.is_empty() {
            let error_msg = format!(
                "-- Error: No SQL query found\n\
                 -- Write your SQL query to: {}\n",
                active.workspace.sql_file.display()
            );
            active.workspace.write_results(&error_msg)?;
            return Ok(());
        }

        // Strip SQL comments to find the actual command
        let sql_without_comments = Self::strip_sql_comments(sql);

        // Check if this is a meta-command
        let (actual_sql, is_meta_command) =
            if let Some(meta_cmd) = MetaCommand::parse(&sql_without_comments) {
                let generated_sql = meta_cmd
                    .to_sql()
                    .context("Failed to generate SQL from meta-command")?;
                (generated_sql, true)
            } else {
                (sql.to_string(), false)
            };

        // Start timing
        let start = Instant::now();
        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");

        log::info!("Executing query for connection '{}'", name);
        if is_meta_command {
            log::debug!("Meta-command: {} -> {}", sql, actual_sql);
        }

        // Execute query
        let result = active.client.query(&actual_sql, &[]).await;

        let duration = start.elapsed();

        match result {
            Ok(rows) => {
                log::info!(
                    "Query executed successfully: {} rows in {:.3}s",
                    rows.len(),
                    duration.as_secs_f64()
                );

                // Format successful result
                let mut output = String::new();
                output.push_str(&format!("-- Executed at: {}\n", timestamp));
                output.push_str(&format!(
                    "-- Execution time: {:.3}s\n",
                    duration.as_secs_f64()
                ));
                output.push_str(&format!("-- Rows returned: {}\n", rows.len()));
                output.push('\n');

                if rows.is_empty() {
                    output.push_str("(No rows returned)\n");
                } else {
                    // Create table
                    let mut table = Table::new();
                    table.load_preset(UTF8_FULL);

                    // Add header
                    let columns = rows[0].columns();
                    let header: Vec<&str> = columns.iter().map(|col| col.name()).collect();
                    table.set_header(header);

                    // Set padding for all columns (left, right)
                    for i in 0..columns.len() {
                        if let Some(column) = table.column_mut(i) {
                            column.set_padding((0, 1));
                        }
                    }

                    // Add rows
                    for row in &rows {
                        let mut row_data = Vec::new();
                        for (idx, col) in columns.iter().enumerate() {
                            let value = Self::value_to_string(row, idx, col.type_());
                            row_data.push(value);
                        }
                        table.add_row(row_data);
                    }

                    output.push_str(&table.to_string());
                }

                active.workspace.write_results(&output)?;
            }
            Err(e) => {
                // Log the error
                if let Some(db_err) = e.as_db_error() {
                    log::warn!("Query failed: {}", db_err.message());
                } else {
                    log::error!("Query execution error: {}", e);
                }

                // Format error
                let mut output = String::new();
                output.push_str(&format!("-- Executed at: {}\n", timestamp));
                output.push_str(&format!(
                    "-- Execution time: {:.3}s\n",
                    duration.as_secs_f64()
                ));
                output.push('\n');

                // Extract database error message if available
                if let Some(db_err) = e.as_db_error() {
                    output.push_str(&format!("ERROR: {}\n", db_err.message()));
                } else {
                    output.push_str(&format!("ERROR: {}\n", e));
                }

                output.push('\n');
                output.push_str("-- Generated SQL:\n");
                output.push_str(&actual_sql);
                output.push('\n');

                active.workspace.write_results(&output)?;
            }
        }

        Ok(())
    }

    /// Get information about an active connection
    pub async fn get_connection_info(&self, name: &str) -> Option<ConnectionInfo> {
        let connections = self.active_connections.lock().await;

        connections.get(name).map(|active| ConnectionInfo {
            name: active.connection_name.clone(),
            uses_tunnel: active.uses_tunnel,
            local_port: active.local_port,
            workspace: active.workspace.clone(),
        })
    }
}

/// Information about a connection
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    pub name: String,
    pub uses_tunnel: bool,
    pub local_port: Option<u16>,
    pub workspace: Workspace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_sql_comments_simple() {
        let sql = "-- This is a comment\n\\d";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d");
    }

    #[test]
    fn test_strip_sql_comments_multiple_lines() {
        let sql = "-- First comment\n-- Second comment\n\\dt users";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\dt users");
    }

    #[test]
    fn test_strip_sql_comments_inline() {
        let sql = "\\d users -- inline comment";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d users");
    }

    #[test]
    fn test_strip_sql_comments_mixed() {
        let sql = "-- Header comment\n\\dt\n-- Footer comment";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\dt");
    }

    #[test]
    fn test_strip_sql_comments_no_comments() {
        let sql = "\\d users";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d users");
    }

    #[test]
    fn test_strip_sql_comments_regular_query() {
        let sql = "-- Get all users\nSELECT * FROM users;";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "SELECT * FROM users;");
    }

    #[test]
    fn test_strip_sql_comments_multiline() {
        let sql = "/* This is a multiline comment */\n\\d";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d");
    }

    #[test]
    fn test_strip_sql_comments_multiline_spanning() {
        let sql = "/* This is a\nmultiline comment\nspanning multiple lines */\n\\dt users";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\dt users");
    }

    #[test]
    fn test_strip_sql_comments_both_types() {
        let sql = "/* Block comment */\n-- Line comment\n\\d users";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d users");
    }

    #[test]
    fn test_strip_sql_comments_inline_multiline() {
        let sql = "\\d /* inline comment */ users";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\d users");
    }

    #[test]
    fn test_strip_sql_comments_mixed_complex() {
        let sql = "/* Header\ncomment */\n-- Another comment\n\\dt\n-- Footer";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "\\dt");
    }

    #[test]
    fn test_strip_sql_comments_multiline_with_query() {
        let sql = "/* Get all users */\nSELECT * FROM users;";
        let result = ConnectionManager::strip_sql_comments(sql);
        assert_eq!(result, "SELECT * FROM users;");
    }
}
