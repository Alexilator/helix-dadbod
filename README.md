# helix-dadbod

A database interface for the Helix editor, inspired by vim-dadbod, vim-dadbod-ui, and vim-dadbod-ssh.

## Features

- PostgreSQL database connections
- SSH tunnel support with host key verification
- Interactive connection picker in Helix
- Auto-execute queries on save
- PostgreSQL meta-commands (like `\d`, `\dt`, `\l`)
- Split pane layout (SQL editor + results viewer)
- Multiple concurrent connections and SSH tunnels

## Current Status

**Production Ready:**

- Configuration management (TOML config)
- Connection management for PostgreSQL
- SSH tunnel creation using russh (ports 7001-7020)
- SSH host key verification against `~/.ssh/known_hosts`
- Direct and tunneled database connections
- Query execution with result formatting
- Steel Scheme integration for Helix editor

**Limitations:**

- PostgreSQL only (MySQL, SQLite not yet supported)
- SSH key authentication only (no password auth)
- SSH config references (`ssh_config = "host"`) require SSH config file parsing

## Setup

### 0. Prerequisites

Make sure your Helix version is compiled with the Steel plugin system, see [here](https://github.com/mattwparas/helix/blob/steel-event-system/STEEL.md) for instructions.

### 1. Install Plugin

You can directly install it from the git repo using the forge package manager:

```bash
forge pkg install --git https://github.com/Alexilator/helix-dadbod.git
```

Or, clone the repo and install it directly:

```bash
forge install
```

And add the following line to your `init.scm`:

```scheme
(require "helix-dadbod/dadbod.scm")
```

### 2. Configure Database Connections

Copy the `config.toml.example` to `~/.config/helix-dadbod/config.toml`

## Usage in Helix

1. Open Helix
2. Open connection picker: `:db-open-picker`
   3a. Navigate with arrow keys, j/k, or Tab/Shift+Tab and confirm selection with Enter
   3b. Or quickly select connection with one of the number keys (1-9)
3. Write SQL in the upper pane
4. Execute query: save the file or run `:db-execute` (or `:dbe`)
5. View results in the lower pane (auto-reloaded)

## Project Structure

```
src/
├── lib.rs            - Main library interface, global state
├── ffi.rs            - FFI exports for Steel Scheme
├── config.rs         - Configuration parsing (config.toml)
├── connection.rs     - Database connection management
├── tunnel.rs         - SSH tunnel management
├── known_hosts.rs    - SSH host key verification
├── ssh_config.rs     - SSH config file parsing
├── meta_commands.rs  - PostgreSQL meta-command translation
└── workspace.rs      - Temporary workspace management

dadbod.scm           - Steel Scheme plugin for Helix
config.toml.example  - Example configuration file
compose.yml          - Docker Compose for dev PostgreSQL
```

## SSH Tunnel Implementation

SSH tunnels are implemented using the `russh` crate.

### How It Works

1. **Port Allocation**: Each SSH tunnel gets a unique local port (7001-7020)
2. **Host Key Verification**: Verifies server key against `~/.ssh/known_hosts`
3. **SSH Connection**: Establishes SSH connection using public key authentication
4. **Local Listener**: Binds a TCP listener on the allocated port
5. **Forwarding Loop**: For each incoming connection:
   - Opens an SSH channel using `channel_open_direct_tcpip`
   - Bidirectionally forwards data between local socket and SSH channel
6. **Multiple Connections**: Multiple database connections can share the same tunnel

### Security

Host key verification is enabled by default. The SSH server's host key must be in your `~/.ssh/known_hosts` file. Supported formats:

- Plaintext: `hostname ssh-ed25519 AAAAC3...`
- Hashed: `|1|base64salt|base64hash ssh-ed25519 AAAAC3...`
- Non-standard ports: `[hostname]:port ssh-ed25519 AAAAC3...`

To add a host key, connect manually first:

```bash
ssh user@hostname
# Type 'yes' to accept the key
```

**Skipping Host Key Verification (INSECURE):**

For development/testing environments, you can disable host key verification in your config.toml:

```toml
skip_host_key_verification = true
```

**WARNING:** This makes your SSH connections vulnerable to man-in-the-middle attacks. Only use this in trusted networks or for testing purposes.

## Development

### Build

**Important:** This project builds a Steel library (dylib), not a binary.

```bash
cargo steel-lib          # Build and install to ~/.local/share/steel/native/
cargo check              # Fast type checking
cargo clippy             # Lint
cargo fmt                # Format code
```

Note: `cargo build` only builds the debugging binary in `main.rs`, not the library that Helix uses.

### Testing

The project has 25 unit tests covering all core functionality:

```bash
cargo test                              # Run all tests (25 tests)
cargo test -- --nocapture               # Run tests with stdout output
cargo test test_name                    # Run a specific test
cargo test meta_commands::              # Run tests in a specific module
cargo test -- --list                    # List all available tests
```

**Test Coverage by Module:**

- **config.rs** (4 tests)
  - `test_parse_explicit_ssh` - Parse connections with explicit SSH tunnel config
  - `test_parse_ssh_config_ref` - Parse connections referencing SSH config hosts
  - `test_skip_host_key_verification_defaults_to_false` - Verify default value is false
  - `test_skip_host_key_verification_can_be_enabled` - Parse and enable skip flag

- **known_hosts.rs** (3 tests)
  - `test_check_plaintext_host` - Verify plaintext known_hosts entries
  - `test_non_standard_port_format` - Handle `[hostname]:port` format
  - `test_pattern_match` - Match wildcard patterns in known_hosts

- **meta_commands.rs** (7 tests)
  - `test_describe_generates_sql` - Generate SQL for `\d` commands
  - `test_parse_describe_no_param` - Parse `\d` without parameters
  - `test_parse_describe_with_table` - Parse `\d tablename`
  - `test_parse_dt` - Parse `\dt` (list tables)
  - `test_parse_dt_with_pattern` - Parse `\dt pattern*`
  - `test_parse_list_databases` - Parse `\l` (list databases)
  - `test_parse_not_meta_command` - Detect regular SQL (not meta-commands)

- **ssh_config.rs** (5 tests)
  - `test_expand_tilde` - Expand `~` to home directory
  - `test_parse_host_defaults` - Parse SSH config with default values
  - `test_parse_host_from_config` - Extract host configuration
  - `test_parse_host_not_found` - Handle missing host entries
  - `test_parse_multiple_hosts` - Parse multiple Host blocks

- **tunnel.rs** (1 test)
  - `test_find_default_ssh_key` - Find id_rsa or id_ed25519 SSH keys

- **workspace.rs** (4 tests)
  - `test_read_write_query` - Read/write query files
  - `test_workspace_cleanup` - Clean up workspace on drop
  - `test_workspace_creation` - Create temporary workspace directories
  - `test_workspace_preserves_existing_sql` - Preserve existing SQL files

- **lib.rs** (1 test)
  - `test_dadbod_from_config` - Initialize Dadbod from config file

### Logging

Logs are written to `~/.config/helix-dadbod/dadbod.log`. Set the log level in your config.toml:

```toml
log_level = "debug"  # Options: error, warn, info, debug, trace
```

Log levels:

- `error` - Only errors (connection failures, parse errors)
- `warn` - Warnings (missing hosts in known_hosts)
- `info` - Operations (connections, tunnel creation, queries)
- `debug` - Detailed debugging (host key verification steps)
- `trace` - Very verbose (all internal operations)

## Next Steps

Planned features:

- Support all postgres data types, at the moment only:
  - bool
  - string
  - integers and numerics
  - floats
  - uuid
  - timestamps and dates
  - json and jsonb
- MySQL and SQLite support

# Influence

This projects is heavily influenced by [vim-dadbod](https://github.com/tpope/vim-dadbod) by Tim Pope. So shoutout to him
