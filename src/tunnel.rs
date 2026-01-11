use crate::config::SshTunnel;
use crate::ssh_config;
use anyhow::{Context, Result};
use async_trait::async_trait;
use russh::client;
use russh_keys::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Port range for SSH tunnels: 7001-7020
const TUNNEL_PORT_START: u16 = 7001;
const TUNNEL_PORT_END: u16 = 7020;

/// SSH client handler for russh
struct SshClientHandler {
    hostname: String,
    port: u16,
    skip_verification: bool,
}

impl SshClientHandler {
    fn new(hostname: String, port: u16, skip_verification: bool) -> Self {
        Self {
            hostname,
            port,
            skip_verification,
        }
    }
}

#[async_trait]
impl client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &key::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Skip verification if configured to do so (INSECURE)
        if self.skip_verification {
            log::warn!(
                "SECURITY WARNING: Skipping host key verification for {}:{} (skip_host_key_verification is enabled)",
                self.hostname, self.port
            );
            return Ok(true);
        }

        // Verify the server's host key against known_hosts
        match crate::known_hosts::verify_host_key(&self.hostname, self.port, server_public_key) {
            Ok(true) => {
                log::info!("Host key verified successfully for {}:{}", self.hostname, self.port);
                Ok(true)
            }
            Ok(false) => {
                log::error!(
                    "Host key verification failed for {}:{} - host not found in known_hosts",
                    self.hostname, self.port
                );
                Err(russh::Error::UnknownKey)
            }
            Err(e) => {
                log::error!(
                    "Error verifying host key for {}:{}: {}",
                    self.hostname, self.port, e
                );
                Err(russh::Error::UnknownKey)
            }
        }
    }
}

/// Manages SSH tunnels for database connections
pub struct TunnelManager {
    tunnels: Arc<Mutex<HashMap<String, ActiveTunnel>>>,
    port_allocator: Arc<Mutex<PortAllocator>>,
    skip_host_key_verification: bool,
}

/// An active SSH tunnel
pub struct ActiveTunnel {
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    /// Handle to the background task that forwards connections
    _forwarding_task: JoinHandle<()>,
}

/// Allocates local ports for tunnels
struct PortAllocator {
    allocated: HashMap<u16, String>, // port -> connection_name
}

impl PortAllocator {
    fn new() -> Self {
        Self {
            allocated: HashMap::new(),
        }
    }

    fn allocate(&mut self, connection_name: &str) -> Result<u16> {
        // Check if this connection already has a port
        for (port, name) in &self.allocated {
            if name == connection_name {
                return Ok(*port);
            }
        }

        // Find the first available port by trying to bind to it
        for port in TUNNEL_PORT_START..=TUNNEL_PORT_END {
            // Skip if already allocated in our tracker
            if self.allocated.contains_key(&port) {
                log::trace!("Port {} already allocated in this manager", port);
                continue;
            }

            // Try to actually bind to the port to see if it's available
            // This handles the case where another process (e.g., another instance) is using it
            if let Ok(_listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
                // Port is available, allocate it
                log::debug!("Allocated port {} for connection '{}'", port, connection_name);
                self.allocated.insert(port, connection_name.to_string());
                return Ok(port);
            }
            // If bind fails, port is in use by another process, try next one
            log::trace!("Port {} in use by another process, trying next", port);
        }

        anyhow::bail!(
            "No available ports in range {}-{}. All ports are in use.",
            TUNNEL_PORT_START,
            TUNNEL_PORT_END
        )
    }

    fn deallocate(&mut self, port: u16) {
        self.allocated.remove(&port);
    }
}

impl TunnelManager {
    pub fn new(skip_host_key_verification: bool) -> Self {
        Self {
            tunnels: Arc::new(Mutex::new(HashMap::new())),
            port_allocator: Arc::new(Mutex::new(PortAllocator::new())),
            skip_host_key_verification,
        }
    }

    /// Get or create a tunnel for the given connection
    pub async fn get_or_create_tunnel(
        &self,
        connection_name: &str,
        ssh_config: &SshTunnel,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<u16> {
        let mut tunnels = self.tunnels.lock().await;

        // Check if tunnel already exists
        if let Some(tunnel) = tunnels.get(connection_name) {
            return Ok(tunnel.local_port);
        }

        // Allocate a local port
        let mut allocator = self.port_allocator.lock().await;
        let local_port = allocator
            .allocate(connection_name)
            .context("Failed to allocate local port for tunnel")?;
        drop(allocator);

        // Create the tunnel
        let tunnel = self
            .create_tunnel(ssh_config, local_port, remote_host, remote_port)
            .await
            .with_context(|| {
                format!(
                    "Failed to create SSH tunnel for connection '{}' on local port {}",
                    connection_name, local_port
                )
            })?;

        tunnels.insert(connection_name.to_string(), tunnel);

        Ok(local_port)
    }

    /// Actually create and start the SSH tunnel
    async fn create_tunnel(
        &self,
        ssh_config: &SshTunnel,
        local_port: u16,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<ActiveTunnel> {
        match ssh_config {
            SshTunnel::Explicit {
                host,
                port,
                user,
                key_path,
            } => {
                log::info!(
                    "Creating SSH tunnel: {}@{}:{} -> localhost:{} -> {}:{}",
                    user, host, port, local_port, remote_host, remote_port
                );

                let key_file = if let Some(path) = key_path {
                    path.clone()
                } else {
                    // Find the default SSH key (tries id_rsa, id_ed25519)
                    find_default_ssh_key()
                        .context("No SSH key specified and no default key found")?
                };

                log::info!("  Using key: {}", key_file.display());

                // Load the private key
                let private_key = load_secret_key(&key_file, None)
                    .with_context(|| format!("Failed to load SSH key from {}", key_file.display()))?;

                // Create SSH configuration
                let ssh_client_config = client::Config::default();
                let ssh_client_config = Arc::new(ssh_client_config);

                // Connect to SSH server
                log::debug!("Connecting to SSH server {}:{}...", host, port);
                let ssh_handler = SshClientHandler::new(host.clone(), *port, self.skip_host_key_verification);
                let mut ssh_session = client::connect(
                    ssh_client_config,
                    (host.as_str(), *port),
                    ssh_handler,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to connect to SSH server {}:{}. \
                         Possible reasons:\n  \
                         - Network connectivity issues\n  \
                         - Host key verification failed (if skip_host_key_verification=false)\n  \
                         - SSH server unreachable",
                        host, port
                    )
                })?;
                log::debug!("SSH connection established to {}:{}", host, port);

                // Authenticate
                log::debug!("Authenticating as user '{}'...", user);
                ssh_session
                    .authenticate_publickey(user, Arc::new(private_key))
                    .await
                    .with_context(|| {
                        format!(
                            "SSH authentication failed for user '{}'. \
                             Check that:\n  \
                             - The SSH key is correct\n  \
                             - The user '{}' has access to the SSH server\n  \
                             - The public key is in ~/.ssh/authorized_keys on the server",
                            user, user
                        )
                    })?;
                log::debug!("SSH authentication successful");

                // Bind local listener
                log::debug!("Binding to local port {}...", local_port);
                let local_listener = TcpListener::bind(("127.0.0.1", local_port))
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to bind to local port {}. \
                             Port may already be in use.",
                            local_port
                        )
                    })?;
                log::debug!("Local listener bound to 127.0.0.1:{}", local_port);

                log::info!("  Tunnel established on localhost:{}", local_port);

                // Wrap SSH session in Arc for sharing across tasks
                log::debug!("Starting tunnel forwarding task");
                let ssh_session = Arc::new(Mutex::new(ssh_session));

                // Spawn forwarding task
                let remote_host_string = remote_host.to_string();
                let remote_host_for_task = remote_host_string.clone();
                let forwarding_task = tokio::spawn(async move {
                    loop {
                        match local_listener.accept().await {
                            Ok((mut local_socket, _)) => {
                                let remote_host_clone = remote_host_for_task.clone();
                                let ssh_session_clone = Arc::clone(&ssh_session);

                                tokio::spawn(async move {
                                    let session = ssh_session_clone.lock().await;
                                    match session
                                        .channel_open_direct_tcpip(
                                            &remote_host_clone,
                                            remote_port as u32,
                                            "127.0.0.1",
                                            local_port as u32,
                                        )
                                        .await
                                    {
                                        Ok(ssh_channel) => {
                                            drop(session); // Release the lock
                                            let mut ssh_stream = ssh_channel.into_stream();

                                            if let Err(e) = tokio::io::copy_bidirectional(
                                                &mut local_socket,
                                                &mut ssh_stream,
                                            )
                                            .await
                                            {
                                                log::error!("Forwarding error: {}", e);
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to open SSH channel: {}", e);
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                log::error!("Failed to accept local connection: {}", e);
                                break;
                            }
                        }
                    }
                });

                Ok(ActiveTunnel {
                    local_port,
                    remote_host: remote_host_string,
                    remote_port,
                    _forwarding_task: forwarding_task,
                })
            }
            SshTunnel::ConfigRef { ssh_config: config_name } => {
                log::info!(
                    "Creating SSH tunnel using config: {} -> localhost:{} -> {}:{}",
                    config_name, local_port, remote_host, remote_port
                );

                // Parse the SSH config file
                let host_config = ssh_config::parse_ssh_config(config_name)
                    .with_context(|| format!("Failed to parse SSH config for host '{}'", config_name))?;

                log::info!(
                    "  Parsed config: {}@{}:{}",
                    host_config.user.as_deref().unwrap_or("<current user>"),
                    host_config.hostname,
                    host_config.port
                );

                // Determine the user (use current user if not specified in config)
                let user = if let Some(u) = host_config.user {
                    u
                } else {
                    std::env::var("USER")
                        .or_else(|_| std::env::var("USERNAME"))
                        .context("Could not determine username. Please specify User in SSH config or set USER environment variable")?
                };

                // Determine the key file (use specified, or fall back to auto-discovery)
                let key_file = if let Some(path) = host_config.identity_file {
                    path
                } else {
                    find_default_ssh_key()
                        .context("No IdentityFile specified in SSH config and no default key found")?
                };

                log::info!("  Using key: {}", key_file.display());

                // Load the private key
                let private_key = load_secret_key(&key_file, None)
                    .with_context(|| format!("Failed to load SSH key from {}", key_file.display()))?;

                // Create SSH configuration
                let ssh_client_config = client::Config::default();
                let ssh_client_config = Arc::new(ssh_client_config);

                // Connect to SSH server
                let ssh_handler = SshClientHandler::new(host_config.hostname.clone(), host_config.port, self.skip_host_key_verification);
                let mut ssh_session = client::connect(
                    ssh_client_config,
                    (host_config.hostname.as_str(), host_config.port),
                    ssh_handler,
                )
                .await
                .with_context(|| {
                    format!(
                        "Failed to connect to SSH server {}:{}\n\
                         Host key verification failed - connect to the SSH host once from outside helix",
                        host_config.hostname, host_config.port
                    )
                })?;

                // Authenticate
                ssh_session
                    .authenticate_publickey(&user, Arc::new(private_key))
                    .await
                    .context("SSH authentication failed")?;

                // Bind local listener
                let local_listener = TcpListener::bind(("127.0.0.1", local_port))
                    .await
                    .with_context(|| format!("Failed to bind to local port {}", local_port))?;

                log::info!("  Tunnel established on localhost:{}", local_port);

                // Wrap SSH session in Arc for sharing across tasks
                let ssh_session = Arc::new(Mutex::new(ssh_session));

                // Spawn forwarding task
                let remote_host_string = remote_host.to_string();
                let remote_host_for_task = remote_host_string.clone();
                let forwarding_task = tokio::spawn(async move {
                    loop {
                        match local_listener.accept().await {
                            Ok((mut local_socket, _)) => {
                                let remote_host_clone = remote_host_for_task.clone();
                                let ssh_session_clone = Arc::clone(&ssh_session);

                                tokio::spawn(async move {
                                    let session = ssh_session_clone.lock().await;
                                    match session
                                        .channel_open_direct_tcpip(
                                            &remote_host_clone,
                                            remote_port as u32,
                                            "127.0.0.1",
                                            local_port as u32,
                                        )
                                        .await
                                    {
                                        Ok(ssh_channel) => {
                                            drop(session); // Release the lock
                                            let mut ssh_stream = ssh_channel.into_stream();

                                            if let Err(e) = tokio::io::copy_bidirectional(
                                                &mut local_socket,
                                                &mut ssh_stream,
                                            )
                                            .await
                                            {
                                                log::error!("Forwarding error: {}", e);
                                            }
                                        }
                                        Err(e) => {
                                            log::error!("Failed to open SSH channel: {}", e);
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                log::error!("Failed to accept local connection: {}", e);
                                break;
                            }
                        }
                    }
                });

                Ok(ActiveTunnel {
                    local_port,
                    remote_host: remote_host_string,
                    remote_port,
                    _forwarding_task: forwarding_task,
                })
            }
        }
    }

    /// Close a specific tunnel
    pub async fn close_tunnel(&self, connection_name: &str) -> Result<()> {
        let mut tunnels = self.tunnels.lock().await;

        if let Some(tunnel) = tunnels.remove(connection_name) {
            let mut allocator = self.port_allocator.lock().await;
            allocator.deallocate(tunnel.local_port);

            // The forwarding task will be dropped and cancelled automatically
            tunnel._forwarding_task.abort();
            log::info!("Closed tunnel on port {}", tunnel.local_port);
        }

        Ok(())
    }

    /// Close all tunnels
    pub async fn close_all(&self) -> Result<()> {
        let mut tunnels = self.tunnels.lock().await;
        let mut allocator = self.port_allocator.lock().await;

        for (_, tunnel) in tunnels.drain() {
            allocator.deallocate(tunnel.local_port);
            tunnel._forwarding_task.abort();
            log::info!("Closed tunnel on port {}", tunnel.local_port);
        }

        Ok(())
    }

    /// Get the local port for an existing tunnel
    pub async fn get_tunnel_port(&self, connection_name: &str) -> Option<u16> {
        let tunnels = self.tunnels.lock().await;
        tunnels.get(connection_name).map(|t| t.local_port)
    }
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new(false)
    }
}

/// Find the default SSH private key
/// Tries the following keys in order:
/// 1. ~/.ssh/id_rsa
/// 2. ~/.ssh/id_ed25519
fn find_default_ssh_key() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .context("HOME environment variable not set")?;
    let ssh_dir = PathBuf::from(home).join(".ssh");

    // Try common SSH key types in order
    let key_candidates = vec![
        ssh_dir.join("id_rsa"),
        ssh_dir.join("id_ed25519"),
    ];

    for key_path in key_candidates {
        if key_path.exists() {
            return Ok(key_path);
        }
    }

    anyhow::bail!(
        "No SSH private key found. Tried:\n  \
         - ~/.ssh/id_rsa\n  \
         - ~/.ssh/id_ed25519"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_default_ssh_key() {
        // This test will pass if at least one of the default keys exists
        // If neither exists, the function should return an error
        match find_default_ssh_key() {
            Ok(key_path) => {
                // Verify it's one of the expected keys
                let path_str = key_path.to_string_lossy();
                assert!(
                    path_str.ends_with("id_rsa") || path_str.ends_with("id_ed25519"),
                    "Key path should end with id_rsa or id_ed25519, got: {}",
                    path_str
                );
            }
            Err(e) => {
                // If no keys exist, verify the error message mentions both keys
                let error_msg = format!("{}", e);
                assert!(error_msg.contains("id_rsa"));
                assert!(error_msg.contains("id_ed25519"));
            }
        }
    }
}
