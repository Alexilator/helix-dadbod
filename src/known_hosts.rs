use anyhow::{Context, Result};
use russh_keys::key::PublicKey;
use russh_keys::PublicKeyBase64;
use std::fs;
use std::path::PathBuf;

/// Verify a host key against ~/.ssh/known_hosts
pub fn verify_host_key(hostname: &str, port: u16, server_key: &PublicKey) -> Result<bool> {
    let known_hosts_path = get_known_hosts_path()?;

    log::debug!("Verifying host key for {}:{}", hostname, port);
    log::debug!("Known hosts file: {}", known_hosts_path.display());

    if !known_hosts_path.exists() {
        log::warn!(
            "Known hosts file does not exist: {}",
            known_hosts_path.display()
        );
        return Ok(false);
    }

    let contents = fs::read_to_string(&known_hosts_path).with_context(|| {
        format!(
            "Failed to read known_hosts file: {}",
            known_hosts_path.display()
        )
    })?;

    // Normalize hostname with port if non-standard
    let host_pattern = if port == 22 {
        hostname.to_string()
    } else {
        format!("[{}]:{}", hostname, port)
    };

    log::debug!("Looking for host pattern: {}", host_pattern);
    log::debug!("Server key type: {}", server_key.name());
    log::debug!("Server key fingerprint: {}", server_key.fingerprint());

    let mut line_num = 0;
    for line in contents.lines() {
        line_num += 1;
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Parse the line
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            log::debug!("Line {}: Invalid format (< 3 parts)", line_num);
            continue; // Invalid line
        }

        let host_part = parts[0];
        let key_type = parts[1];
        let key_data = parts[2];

        // Check if this entry matches our hostname
        let matches = if host_part.starts_with("|1|") {
            // Hashed format: |1|salt|hash
            log::debug!("Line {}: Checking hashed host entry", line_num);
            match check_hashed_host(&host_pattern, host_part) {
                Ok(m) => {
                    log::debug!("Line {}: Hashed host match: {}", line_num, m);
                    m
                }
                Err(e) => {
                    log::debug!("Line {}: Error checking hashed host: {}", line_num, e);
                    false
                }
            }
        } else {
            // Plaintext format: hostname or hostname,hostname2 or pattern
            log::debug!("Line {}: Checking plaintext host: {}", line_num, host_part);
            let m = check_plaintext_host(&host_pattern, host_part);
            log::debug!("Line {}: Plaintext host match: {}", line_num, m);
            m
        };

        if matches {
            log::debug!(
                "Line {}: Host matched! Checking key type: {}",
                line_num,
                key_type
            );
            // Try to parse the key and compare
            match parse_public_key(key_type, key_data) {
                Ok(known_key) => {
                    log::debug!("Line {}: Known key type: {}", line_num, known_key.name());
                    log::debug!(
                        "Line {}: Known key fingerprint: {}",
                        line_num,
                        known_key.fingerprint()
                    );
                    if keys_match(server_key, &known_key) {
                        log::info!("Host key verified successfully on line {}", line_num);
                        return Ok(true);
                    } else {
                        log::debug!("Line {}: Key mismatch (different fingerprints)", line_num);
                    }
                }
                Err(e) => {
                    log::debug!("Line {}: Failed to parse known key: {}", line_num, e);
                }
            }
        }
    }

    log::warn!(
        "No matching host key found in known_hosts for {}",
        host_pattern
    );
    Ok(false)
}

/// Get the path to the known_hosts file
fn get_known_hosts_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".ssh").join("known_hosts"))
}

/// Check if a plaintext host pattern matches
fn check_plaintext_host(hostname: &str, pattern: &str) -> bool {
    // Handle comma-separated hosts
    for host in pattern.split(',') {
        if host == hostname {
            return true;
        }
        // Handle wildcards (* and ?)
        if pattern_match(hostname, host) {
            return true;
        }
    }
    false
}

/// Simple wildcard pattern matching
fn pattern_match(hostname: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if !pattern.contains('*') && !pattern.contains('?') {
        return hostname == pattern;
    }

    // Convert glob pattern to regex-like matching
    let mut pattern_chars = pattern.chars().peekable();
    let mut hostname_chars = hostname.chars().peekable();

    while pattern_chars.peek().is_some() || hostname_chars.peek().is_some() {
        match pattern_chars.peek() {
            Some('*') => {
                pattern_chars.next();
                if pattern_chars.peek().is_none() {
                    return true; // * at end matches everything
                }
                // Try to match the rest
                while hostname_chars.peek().is_some() {
                    if pattern_match(
                        &hostname_chars.clone().collect::<String>(),
                        &pattern_chars.clone().collect::<String>(),
                    ) {
                        return true;
                    }
                    hostname_chars.next();
                }
                return false;
            }
            Some('?') => {
                pattern_chars.next();
                if hostname_chars.next().is_none() {
                    return false;
                }
            }
            Some(&pc) => {
                pattern_chars.next();
                match hostname_chars.next() {
                    Some(hc) if hc == pc => continue,
                    _ => return false,
                }
            }
            None => return hostname_chars.peek().is_none(),
        }
    }

    true
}

/// Check if a hashed host entry matches
/// Format: |1|salt_base64|hash_base64
fn check_hashed_host(hostname: &str, hashed_entry: &str) -> Result<bool> {
    let parts: Vec<&str> = hashed_entry.split('|').collect();
    if parts.len() != 4 || !parts[0].is_empty() || parts[1] != "1" {
        return Ok(false); // Invalid format
    }

    let salt_b64 = parts[2];
    let expected_hash_b64 = parts[3];

    // Decode the salt
    use base64::Engine;
    let salt = base64::engine::general_purpose::STANDARD
        .decode(salt_b64)
        .context("Failed to decode salt from hashed known_hosts entry")?;

    // Compute HMAC-SHA1 of hostname with salt
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt)
        .map_err(|e| anyhow::anyhow!("Failed to create HMAC: {}", e))?;
    mac.update(hostname.as_bytes());
    let result = mac.finalize();
    let computed_hash = result.into_bytes();

    // Encode as base64
    let computed_hash_b64 = base64::engine::general_purpose::STANDARD.encode(computed_hash);

    Ok(computed_hash_b64 == expected_hash_b64)
}

/// Parse a public key from known_hosts format
fn parse_public_key(key_type: &str, key_data: &str) -> Result<PublicKey> {
    use base64::Engine;

    // Decode the base64 key data
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(key_data)
        .with_context(|| format!("Failed to decode base64 key data for type {}", key_type))?;

    // Parse the key bytes based on type
    russh_keys::key::parse_public_key(&key_bytes, None)
        .with_context(|| format!("Failed to parse public key of type {}", key_type))
}

/// Compare two public keys for equality
fn keys_match(key1: &PublicKey, key2: &PublicKey) -> bool {
    // Compare the keys by encoding them as base64 and comparing
    // This is the most reliable way to compare keys across different types
    key1.public_key_base64() == key2.public_key_base64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_match() {
        assert!(pattern_match("example.com", "example.com"));
        assert!(pattern_match("example.com", "*.com"));
        assert!(pattern_match("example.com", "example.*"));
        assert!(pattern_match("example.com", "*"));
        assert!(pattern_match("example.com", "ex?mple.com"));

        assert!(!pattern_match("example.com", "example.org"));
        assert!(!pattern_match("example.com", "*.org"));
    }

    #[test]
    fn test_check_plaintext_host() {
        assert!(check_plaintext_host("example.com", "example.com"));
        assert!(check_plaintext_host("example.com", "example.com,other.com"));
        assert!(check_plaintext_host("example.com", "*.com"));

        assert!(!check_plaintext_host("example.com", "other.com"));
        assert!(!check_plaintext_host("example.com", "example.org"));
    }

    #[test]
    fn test_non_standard_port_format() {
        // Test that non-standard ports use bracket notation
        assert!(check_plaintext_host(
            "[example.com]:2222",
            "[example.com]:2222"
        ));
        assert!(!check_plaintext_host("[example.com]:2222", "example.com"));
        assert!(!check_plaintext_host("example.com", "[example.com]:2222"));
    }
}
