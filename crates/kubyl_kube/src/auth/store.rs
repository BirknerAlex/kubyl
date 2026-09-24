//! Where refresh tokens are kept: the OS keychain (macOS Keychain, Windows Credential Manager,
//! Secret Service on Linux) via `keyring`.
//!
//! Unsigned dev builds make macOS ask for keychain access on every rebuild. Setting
//! `KUBYL_CREDENTIAL_STORE=file` switches to a plain JSON file
//! (`<config dir>/dev-credentials.json`, mode 0600) for development. Never use it for real
//! clusters. `KUBYL_CREDENTIAL_STORE=memory` keeps secrets only for the current run (tests).
//!
//! All calls block (the keychain may show a prompt), so run them on the Tokio runtime's
//! blocking pool, never on the UI thread.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;
use secrecy::{ExposeSecret as _, SecretString};

/// Keychain service name for every entry.
const SERVICE: &str = "io.github.birkneralex.Kubyl";

enum Backend {
    Keyring,
    File(PathBuf),
    Memory(Mutex<BTreeMap<String, String>>),
}

fn backend() -> &'static Backend {
    static BACKEND: OnceLock<Backend> = OnceLock::new();
    BACKEND.get_or_init(
        || match std::env::var("KUBYL_CREDENTIAL_STORE").as_deref() {
            Ok("file") => {
                let path = kubyl_settings::config_dir().join("dev-credentials.json");
                tracing::warn!(
                    "KUBYL_CREDENTIAL_STORE=file: storing credentials in plain text at {}",
                    path.display()
                );
                Backend::File(path)
            }
            Ok("memory") => Backend::Memory(Mutex::default()),
            _ => Backend::Keyring,
        },
    )
}

/// A human-readable name of the active store, for the UI.
pub fn store_name() -> &'static str {
    match backend() {
        Backend::Keyring if cfg!(target_os = "macos") => "Keychain",
        Backend::Keyring if cfg!(target_os = "windows") => "Credential Manager",
        Backend::Keyring => "Secret Service",
        Backend::File(_) => "dev file store",
        Backend::Memory(_) => "memory",
    }
}

/// Reads a secret. `Ok(None)` when there is none.
pub fn get(key: &str) -> Result<Option<SecretString>, String> {
    match backend() {
        Backend::Keyring => {
            let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
            match entry.get_password() {
                Ok(secret) => Ok(Some(SecretString::from(secret))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(err.to_string()),
            }
        }
        Backend::File(path) => Ok(read_file(path).remove(key).map(SecretString::from)),
        Backend::Memory(map) => Ok(map.lock().get(key).cloned().map(SecretString::from)),
    }
}

/// Stores a secret, replacing an existing one.
pub fn set(key: &str, secret: &SecretString) -> Result<(), String> {
    match backend() {
        Backend::Keyring => keyring::Entry::new(SERVICE, key)
            .and_then(|entry| entry.set_password(secret.expose_secret()))
            .map_err(|e| e.to_string()),
        Backend::File(path) => {
            let mut map = read_file(path);
            map.insert(key.to_string(), secret.expose_secret().to_string());
            write_file(path, &map).map_err(|e| e.to_string())
        }
        Backend::Memory(map) => {
            map.lock()
                .insert(key.to_string(), secret.expose_secret().to_string());
            Ok(())
        }
    }
}

/// Removes a secret. Missing entries are fine.
pub fn delete(key: &str) -> Result<(), String> {
    match backend() {
        Backend::Keyring => {
            let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err.to_string()),
            }
        }
        Backend::File(path) => {
            let mut map = read_file(path);
            if map.remove(key).is_some() {
                write_file(path, &map).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Backend::Memory(map) => {
            map.lock().remove(key);
            Ok(())
        }
    }
}

fn read_file(path: &PathBuf) -> BTreeMap<String, String> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_file(path: &PathBuf, map: &BTreeMap<String, String>) -> std::io::Result<()> {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&serde_json::to_vec_pretty(map).expect("serialize credentials"))?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}
