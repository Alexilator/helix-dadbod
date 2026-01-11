//! PostgreSQL meta-command parser and SQL generator
//!
//! Translates psql-style meta-commands (like \d, \dt, etc.) into equivalent
//! SQL queries against PostgreSQL's system catalogs.

use anyhow::Result;

/// Represents a parsed PostgreSQL meta-command
#[derive(Debug, PartialEq)]
pub enum MetaCommand {
    /// \d [table] - List all tables, or describe specific table
    Describe(Option<String>),
    /// \dt [pattern] - List tables
    DescribeTables(Option<String>),
    /// \dv [pattern] - List views
    DescribeViews(Option<String>),
    /// \di [pattern] - List indexes
    DescribeIndexes(Option<String>),
    /// \ds [pattern] - List sequences
    DescribeSequences(Option<String>),
    /// \df [pattern] - List functions
    DescribeFunctions(Option<String>),
    /// \dn [pattern] - List schemas
    DescribeSchemas(Option<String>),
    /// \l - List databases
    ListDatabases,
    /// \du - List users/roles
    DescribeUsers,
}

impl MetaCommand {
    /// Parse a SQL string and detect if it's a meta-command
    pub fn parse(sql: &str) -> Option<Self> {
        let trimmed = sql.trim();

        // Must start with backslash
        if !trimmed.starts_with('\\') {
            return None;
        }

        // Split into command and optional parameter
        let parts: Vec<&str> = trimmed[1..].split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        let command = parts[0];
        let param = if parts.len() > 1 {
            Some(parts[1].to_string())
        } else {
            None
        };

        match command {
            "d" => Some(MetaCommand::Describe(param)),
            "dt" => Some(MetaCommand::DescribeTables(param)),
            "dv" => Some(MetaCommand::DescribeViews(param)),
            "di" => Some(MetaCommand::DescribeIndexes(param)),
            "ds" => Some(MetaCommand::DescribeSequences(param)),
            "df" => Some(MetaCommand::DescribeFunctions(param)),
            "dn" => Some(MetaCommand::DescribeSchemas(param)),
            "l" => Some(MetaCommand::ListDatabases),
            "du" => Some(MetaCommand::DescribeUsers),
            _ => None,
        }
    }

    /// Generate the equivalent SQL query for this meta-command
    pub fn to_sql(&self) -> Result<String> {
        match self {
            MetaCommand::Describe(None) => {
                // \d without parameter - list all tables (same as \dt)
                Ok(Self::list_tables_sql(None))
            }
            MetaCommand::Describe(Some(table)) => {
                // \d tablename - describe specific table
                Ok(Self::describe_table_sql(table))
            }
            MetaCommand::DescribeTables(pattern) => Ok(Self::list_tables_sql(pattern.as_deref())),
            MetaCommand::DescribeViews(pattern) => Ok(Self::list_views_sql(pattern.as_deref())),
            MetaCommand::DescribeIndexes(pattern) => Ok(Self::list_indexes_sql(pattern.as_deref())),
            MetaCommand::DescribeSequences(pattern) => {
                Ok(Self::list_sequences_sql(pattern.as_deref()))
            }
            MetaCommand::DescribeFunctions(pattern) => {
                Ok(Self::list_functions_sql(pattern.as_deref()))
            }
            MetaCommand::DescribeSchemas(pattern) => Ok(Self::list_schemas_sql(pattern.as_deref())),
            MetaCommand::ListDatabases => Ok(Self::list_databases_sql()),
            MetaCommand::DescribeUsers => Ok(Self::list_users_sql()),
        }
    }

    /// Generate SQL to list all tables
    fn list_tables_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND c.relname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Schema\",
  c.relname AS \"Name\",
  CASE c.relkind
    WHEN 'r' THEN 'table'
    WHEN 'p' THEN 'partitioned table'
  END AS \"Type\",
  pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\"
FROM pg_catalog.pg_class c
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r', 'p')
  AND n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
  AND n.nspname !~ '^pg_toast'
{}ORDER BY 1, 2;",
            where_clause
        )
    }

    /// Generate SQL to describe a specific table
    fn describe_table_sql(table: &str) -> String {
        let escaped_table = table.replace('\'', "''");

        format!(
            "SELECT
  a.attname AS \"Column\",
  pg_catalog.format_type(a.atttypid, a.atttypmod) AS \"Type\",
  CASE
    WHEN a.attnotnull THEN 'NOT NULL'
    ELSE ''
  END AS \"Nullable\",
  CASE
    WHEN a.atthasdef THEN pg_catalog.pg_get_expr(d.adbin, d.adrelid)
    ELSE ''
  END AS \"Default\"
FROM pg_catalog.pg_attribute a
LEFT JOIN pg_catalog.pg_attrdef d ON (a.attrelid, a.attnum) = (d.adrelid, d.adnum)
WHERE a.attrelid = '{}'::regclass
  AND a.attnum > 0
  AND NOT a.attisdropped
ORDER BY a.attnum;",
            escaped_table
        )
    }

    /// Generate SQL to list views
    fn list_views_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND c.relname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Schema\",
  c.relname AS \"Name\",
  CASE c.relkind
    WHEN 'v' THEN 'view'
    WHEN 'm' THEN 'materialized view'
  END AS \"Type\",
  pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\"
FROM pg_catalog.pg_class c
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('v', 'm')
  AND n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
{}ORDER BY 1, 2;",
            where_clause
        )
    }

    /// Generate SQL to list indexes
    fn list_indexes_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND c.relname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Schema\",
  c.relname AS \"Name\",
  pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\",
  t.relname AS \"Table\"
FROM pg_catalog.pg_class c
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_catalog.pg_index i ON i.indexrelid = c.oid
LEFT JOIN pg_catalog.pg_class t ON i.indrelid = t.oid
WHERE c.relkind = 'i'
  AND n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
{}ORDER BY 1, 2;",
            where_clause
        )
    }

    /// Generate SQL to list sequences
    fn list_sequences_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND c.relname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Schema\",
  c.relname AS \"Name\",
  pg_catalog.pg_get_userbyid(c.relowner) AS \"Owner\"
FROM pg_catalog.pg_class c
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind = 'S'
  AND n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
{}ORDER BY 1, 2;",
            where_clause
        )
    }

    /// Generate SQL to list functions
    fn list_functions_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND p.proname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Schema\",
  p.proname AS \"Name\",
  pg_catalog.pg_get_function_result(p.oid) AS \"Result data type\",
  pg_catalog.pg_get_function_arguments(p.oid) AS \"Argument data types\"
FROM pg_catalog.pg_proc p
LEFT JOIN pg_catalog.pg_namespace n ON n.oid = p.pronamespace
WHERE n.nspname <> 'pg_catalog'
  AND n.nspname <> 'information_schema'
{}ORDER BY 1, 2;",
            where_clause
        )
    }

    /// Generate SQL to list schemas
    fn list_schemas_sql(pattern: Option<&str>) -> String {
        let where_clause = if let Some(p) = pattern {
            format!("  AND n.nspname LIKE '%{}%'\n", p.replace('\'', "''"))
        } else {
            String::new()
        };

        format!(
            "SELECT n.nspname AS \"Name\",
  pg_catalog.pg_get_userbyid(n.nspowner) AS \"Owner\"
FROM pg_catalog.pg_namespace n
WHERE n.nspname !~ '^pg_'
  AND n.nspname <> 'information_schema'
{}ORDER BY 1;",
            where_clause
        )
    }

    /// Generate SQL to list databases
    fn list_databases_sql() -> String {
        "SELECT d.datname AS \"Name\",
  pg_catalog.pg_get_userbyid(d.datdba) AS \"Owner\",
  pg_catalog.pg_encoding_to_char(d.encoding) AS \"Encoding\",
  d.datcollate AS \"Collate\",
  d.datctype AS \"Ctype\"
FROM pg_catalog.pg_database d
ORDER BY 1;"
            .to_string()
    }

    /// Generate SQL to list users/roles
    fn list_users_sql() -> String {
        "SELECT r.rolname AS \"Role name\",
  CASE
    WHEN r.rolsuper THEN 'Superuser'
    ELSE ''
  END AS \"Attributes\",
  ARRAY(
    SELECT b.rolname
    FROM pg_catalog.pg_auth_members m
    JOIN pg_catalog.pg_roles b ON (m.roleid = b.oid)
    WHERE m.member = r.oid
  ) AS \"Member of\"
FROM pg_catalog.pg_roles r
WHERE r.rolname !~ '^pg_'
ORDER BY 1;"
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_describe_no_param() {
        let cmd = MetaCommand::parse("\\d");
        assert_eq!(cmd, Some(MetaCommand::Describe(None)));
    }

    #[test]
    fn test_parse_describe_with_table() {
        let cmd = MetaCommand::parse("\\d users");
        assert_eq!(cmd, Some(MetaCommand::Describe(Some("users".to_string()))));
    }

    #[test]
    fn test_parse_dt() {
        let cmd = MetaCommand::parse("\\dt");
        assert_eq!(cmd, Some(MetaCommand::DescribeTables(None)));
    }

    #[test]
    fn test_parse_dt_with_pattern() {
        let cmd = MetaCommand::parse("\\dt user");
        assert_eq!(
            cmd,
            Some(MetaCommand::DescribeTables(Some("user".to_string())))
        );
    }

    #[test]
    fn test_parse_list_databases() {
        let cmd = MetaCommand::parse("\\l");
        assert_eq!(cmd, Some(MetaCommand::ListDatabases));
    }

    #[test]
    fn test_parse_not_meta_command() {
        let cmd = MetaCommand::parse("SELECT * FROM users");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_describe_generates_sql() {
        let cmd = MetaCommand::Describe(Some("users".to_string()));
        let sql = cmd.to_sql().unwrap();
        assert!(sql.contains("pg_catalog.pg_attribute"));
        assert!(sql.contains("'users'::regclass"));
    }

    #[test]
    fn test_parse_with_leading_whitespace() {
        let cmd = MetaCommand::parse("   \\d   ");
        assert_eq!(cmd, Some(MetaCommand::Describe(None)));
    }

    #[test]
    fn test_parse_dt_after_comment_stripped() {
        // This tests the scenario after SQL comments have been stripped
        let cmd = MetaCommand::parse("\\dt");
        assert_eq!(cmd, Some(MetaCommand::DescribeTables(None)));
    }
}
