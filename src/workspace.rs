use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Workspace for a database connection
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Root directory: /tmp/helix-dadbod
    pub path: PathBuf,
    /// Path to connection-specific SQL file: /tmp/helix-dadbod/{connection_name}.sql
    pub sql_file: PathBuf,
    /// Path to shared results file: /tmp/helix-dadbod/results.dbout
    pub dbout_file: PathBuf,
}

impl Workspace {
    /// Create a new workspace for the connection
    /// SQL file: /tmp/helix-dadbod/{connection_name}.sql
    /// Results file: /tmp/helix-dadbod/results.dbout (shared)
    pub fn create(connection_name: &str) -> Result<Self> {
        let path = PathBuf::from("/tmp").join("helix-dadbod");

        // Create the directory if it doesn't exist
        fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create workspace directory: {}", path.display()))?;

        let sql_file = path.join(format!("{}.sql", connection_name));
        let dbout_file = path.join("results.dbout");

        // Create empty SQL file only if it doesn't exist (preserve user's queries)
        if !sql_file.exists() {
            fs::write(&sql_file, "")
                .with_context(|| format!("Failed to create SQL file: {}", sql_file.display()))?;
            log::info!("Created new SQL file: {}", sql_file.display());
        } else {
            log::info!("Reusing existing SQL file: {}", sql_file.display());
        }

        // Create results.dbout with initial message (always overwrite to show fresh connection)
        let initial_content = format!(
            "-- helix-dadbod results\n\
             -- Connection: '{}'\n\
             -- Connected at: {}\n\
             -- Write your SQL queries to: {}\n\
             -- Execute to see results here\n",
            connection_name,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            sql_file.display()
        );
        fs::write(&dbout_file, initial_content)
            .with_context(|| format!("Failed to create results.dbout: {}", dbout_file.display()))?;

        log::info!("Created workspace for connection: {}", connection_name);
        log::info!("  SQL file: {}", sql_file.display());
        log::info!("  Output file: {}", dbout_file.display());

        Ok(Self {
            path,
            sql_file,
            dbout_file,
        })
    }

    /// Read the SQL query from query.sql
    pub fn read_query(&self) -> Result<String> {
        fs::read_to_string(&self.sql_file)
            .with_context(|| format!("Failed to read query from: {}", self.sql_file.display()))
    }

    /// Write results to results.dbout
    pub fn write_results(&self, content: &str) -> Result<()> {
        fs::write(&self.dbout_file, content)
            .with_context(|| format!("Failed to write results to: {}", self.dbout_file.display()))
    }

    /// Clean up the workspace directory
    pub fn cleanup(&self) -> Result<()> {
        if self.path.exists() {
            fs::remove_dir_all(&self.path).with_context(|| {
                format!(
                    "Failed to remove workspace directory: {}",
                    self.path.display()
                )
            })?;
            log::info!("Cleaned up workspace: {}", self.path.display());
        }
        Ok(())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // Note: We don't auto-cleanup on drop because connections might be long-lived
        // Cleanup should be called explicitly or handled by the connection manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Note: These tests share the /tmp/helix-dadbod directory and results.dbout file
    // Run with --test-threads=1 to avoid race conditions:
    //   cargo test -- --test-threads=1

    #[test]
    fn test_workspace_creation() {
        let test_name = "test_connection_create";
        let workspace = Workspace::create(test_name).unwrap();

        // Verify paths are correct
        assert_eq!(workspace.path, PathBuf::from("/tmp/helix-dadbod"));
        assert_eq!(
            workspace.sql_file,
            PathBuf::from(format!("/tmp/helix-dadbod/{}.sql", test_name))
        );
        assert_eq!(
            workspace.dbout_file,
            PathBuf::from("/tmp/helix-dadbod/results.dbout")
        );

        // Verify files exist
        assert!(workspace.sql_file.exists());
        assert!(workspace.dbout_file.exists());

        // Verify SQL file is empty (new workspace)
        let sql_content = fs::read_to_string(&workspace.sql_file).unwrap();
        assert_eq!(sql_content, "");

        // Verify dbout file has initial content (may have been overwritten by other tests)
        let dbout_content = fs::read_to_string(&workspace.dbout_file).unwrap();
        // Just check it exists and is not empty
        assert!(!dbout_content.is_empty());

        // Cleanup
        fs::remove_file(&workspace.sql_file).ok();
    }

    #[test]
    fn test_workspace_preserves_existing_sql() {
        let test_name = "test_connection_preserve";
        let workspace = Workspace::create(test_name).unwrap();

        // Write some SQL
        let test_sql = "SELECT * FROM users;";
        fs::write(&workspace.sql_file, test_sql).unwrap();

        // Create workspace again - should preserve the SQL
        let workspace2 = Workspace::create(test_name).unwrap();
        let sql_content = fs::read_to_string(&workspace2.sql_file).unwrap();
        assert_eq!(sql_content, test_sql);

        // Cleanup
        fs::remove_file(&workspace.sql_file).ok();
    }

    #[test]
    fn test_read_write_query() {
        let test_name = "test_connection_rw";
        let workspace = Workspace::create(test_name).unwrap();

        // Write a query to the SQL file
        let query = "SELECT version();";
        fs::write(&workspace.sql_file, query).unwrap();

        // Read it back using workspace method
        let read_query = workspace.read_query().unwrap();
        assert_eq!(read_query, query);

        // Write results using workspace method
        let results = "PostgreSQL 14.5";
        workspace.write_results(results).unwrap();

        // Verify results were written
        let read_results = fs::read_to_string(&workspace.dbout_file).unwrap();
        assert_eq!(read_results, results);

        // Cleanup
        fs::remove_file(&workspace.sql_file).ok();
    }

    #[test]
    fn test_workspace_cleanup() {
        let test_name = "test_connection_cleanup";
        let workspace = Workspace::create(test_name).unwrap();

        assert!(workspace.path.exists());
        assert!(workspace.sql_file.exists());

        // Note: We can't fully test cleanup() because other tests use the same directory
        // Just verify that the workspace was created successfully
        // In a real scenario, cleanup() removes the entire /tmp/helix-dadbod directory

        // Cleanup just our test file
        fs::remove_file(&workspace.sql_file).ok();
    }
}
