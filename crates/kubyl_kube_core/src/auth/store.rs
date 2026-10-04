//! Where refresh tokens are kept: the OS keychain (macOS Keychain, Windows Credential Manager,
//! Secret Service on Linux) via `keyring`.
//!
//! On macOS, release builds carry a provisioning profile that grants the `keychain-access-groups`
//! entitlement (`packaging/macos/entitlements.plist`), so they use the data protection keychain:
//! items belong to Kubyl's access group (team ID + bundle ID) and macOS never asks, not even after
//! an update. Builds without the entitlement (`cargo run`, local builds) fall back to the login
//! keychain, which asks per item whenever the binary changes (details on the `keychain` module).
//!
//! Unsigned dev builds make macOS ask for keychain access on every rebuild. Setting
//! `KUBYL_CREDENTIAL_STORE=file` switches to a plain JSON file
//! (`<config dir>/dev-credentials.json`, mode 0600) for development. Never use it for real
//! clusters. `KUBYL_CREDENTIAL_STORE=memory` keeps secrets only for the current run (tests).
//!
//! An app can install its own backend with [`install`] (once, before the first call), e.g. an
//! encrypted file on a machine without a keychain. It replaces all of the above, including
//! `KUBYL_CREDENTIAL_STORE`.
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

/// A credential store an app installs with [`install`]. Calls block, like the built-in stores;
/// they run on the Tokio runtime's blocking pool. Implementations must never log a secret.
pub trait SecretStore: Send + Sync {
    /// A human-readable name, for the UI.
    fn name(&self) -> &'static str;
    /// Reads a secret. `Ok(None)` when there is none.
    fn get(&self, key: &str) -> Result<Option<SecretString>, String>;
    /// Stores a secret, replacing an existing one.
    fn set(&self, key: &str, secret: &SecretString) -> Result<(), String>;
    /// Removes a secret. Missing entries are fine.
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The store was already chosen: by an earlier [`install`], or by the first call that read,
/// wrote or named the store.
#[derive(Debug, thiserror::Error)]
#[error("the credential store is already in use")]
pub struct AlreadyInUse;

enum Backend {
    Keyring,
    File(PathBuf),
    Memory(Mutex<BTreeMap<String, String>>),
    Custom(Box<dyn SecretStore>),
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

/// Installs the app's own credential store. Call it once at startup, before anything reads or
/// writes a credential; without it the store is the OS keychain, or what
/// `KUBYL_CREDENTIAL_STORE` selects.
pub fn install(store: Box<dyn SecretStore>) -> Result<(), AlreadyInUse> {
    BACKEND
        .set(Backend::Custom(store))
        .map_err(|_| AlreadyInUse)
}

fn backend() -> &'static Backend {
    BACKEND.get_or_init(|| from_env(std::env::var("KUBYL_CREDENTIAL_STORE").ok().as_deref()))
}

fn from_env(value: Option<&str>) -> Backend {
    match value {
        Some("file") => {
            let path = kubyl_settings_core::config_dir().join("dev-credentials.json");
            tracing::warn!(
                "KUBYL_CREDENTIAL_STORE=file: storing credentials in plain text at {}",
                path.display()
            );
            Backend::File(path)
        }
        Some("memory") => Backend::Memory(Mutex::default()),
        _ => Backend::Keyring,
    }
}

/// A human-readable name of the active store, for the UI.
pub fn store_name() -> &'static str {
    match backend() {
        Backend::Custom(store) => store.name(),
        Backend::Keyring if cfg!(target_os = "macos") => "Keychain",
        Backend::Keyring if cfg!(target_os = "windows") => "Credential Manager",
        Backend::Keyring => "Secret Service",
        Backend::File(_) => "dev file store",
        Backend::Memory(_) => "memory",
    }
}

/// Reads a secret. `Ok(None)` when there is none.
pub fn get(key: &str) -> Result<Option<SecretString>, String> {
    get_from(backend(), key)
}

fn get_from(backend: &Backend, key: &str) -> Result<Option<SecretString>, String> {
    match backend {
        Backend::Custom(store) => store.get(key),
        Backend::Keyring => {
            let entry = entry(key).map_err(|e| e.to_string())?;
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
    set_in(backend(), key, secret)
}

fn set_in(backend: &Backend, key: &str, secret: &SecretString) -> Result<(), String> {
    match backend {
        Backend::Custom(store) => store.set(key, secret),
        Backend::Keyring => entry(key)
            .and_then(|entry| entry.set_password(secret.expose_secret()))
            .map_err(|e| e.to_string()),
        Backend::File(path) => update_file(path, |map| {
            map.insert(key.to_string(), secret.expose_secret().to_string());
            true
        })
        .map_err(|e| e.to_string()),
        Backend::Memory(map) => {
            map.lock()
                .insert(key.to_string(), secret.expose_secret().to_string());
            Ok(())
        }
    }
}

/// Removes a secret. Missing entries are fine.
pub fn delete(key: &str) -> Result<(), String> {
    delete_in(backend(), key)
}

fn delete_in(backend: &Backend, key: &str) -> Result<(), String> {
    match backend {
        Backend::Custom(store) => store.delete(key),
        Backend::Keyring => {
            let entry = entry(key).map_err(|e| e.to_string())?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(err.to_string()),
            }
        }
        Backend::File(path) => {
            update_file(path, |map| map.remove(key).is_some()).map_err(|e| e.to_string())
        }
        Backend::Memory(map) => {
            map.lock().remove(key);
            Ok(())
        }
    }
}

/// The OS keychain entry of `key`.
fn entry(key: &str) -> keyring::Result<keyring::Entry> {
    #[cfg(target_os = "macos")]
    if keychain::protected() {
        return keychain::entry(key);
    }
    keyring::Entry::new(SERVICE, key)
}

/// macOS: the data protection keychain for builds with a keychain access group, else the login
/// keychain.
///
/// The login keychain trusts apps by code signature and asks (with the keychain password) for
/// every other one: `cargo` builds are only ad-hoc signed, so each rebuild is a new app, and
/// writing an item resets its partition list to the writer. Release builds don't read what
/// earlier releases left there; sign in again once.
#[cfg(target_os = "macos")]
mod keychain {
    use std::path::Path;
    use std::sync::OnceLock;

    use apple_native_keyring_store::protected::{AccessPolicy, Cred};

    use super::SERVICE;

    /// Whether this build may use the data protection keychain, checked once. macOS only starts a
    /// binary with the `keychain-access-groups` entitlement when an embedded provisioning profile
    /// grants it, and release.yml adds both together. The keychain can't tell: without the
    /// entitlement, reads there just find nothing (only writes fail, with
    /// `errSecMissingEntitlement`). The code-signing API can, but takes seconds.
    pub(super) fn protected() -> bool {
        static PROTECTED: OnceLock<bool> = OnceLock::new();
        *PROTECTED.get_or_init(|| {
            let profiled = std::env::current_exe()
                .and_then(|exe| exe.canonicalize())
                .is_ok_and(|exe| profiled(&exe));
            if !profiled {
                tracing::info!(
                    "keychain: no provisioning profile (not a release build), using the login keychain"
                );
            }
            profiled
        })
    }

    /// Whether `exe` (`Kubyl.app/Contents/MacOS/kubyl`) has a bundle with a provisioning profile.
    pub(super) fn profiled(exe: &Path) -> bool {
        exe.parent()
            .and_then(Path::parent)
            .is_some_and(|contents| contents.join("embedded.provisionprofile").is_file())
    }

    /// An item in the app's default access group. Readable after the first unlock (tokens refresh
    /// while the screen is locked), never synced or restored to another Mac.
    pub(super) fn entry(key: &str) -> keyring::Result<keyring::Entry> {
        Cred::build(
            SERVICE,
            key,
            AccessPolicy::AfterFirstUnlockThisDeviceOnly,
            None,
            false,
        )
        .map(|inner| keyring::Entry { inner })
    }
}

/// Reads the file, applies `change` and writes it back if `change` returned true. Serialized,
/// so concurrent writers (the token stores of several clusters) don't lose each other's
/// entries.
fn update_file(
    path: &PathBuf,
    change: impl FnOnce(&mut BTreeMap<String, String>) -> bool,
) -> std::io::Result<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock();
    let mut map = read_file(path);
    if change(&mut map) {
        write_file(path, &map)?;
    }
    Ok(())
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::keychain;

    #[test]
    fn only_bundles_with_a_provisioning_profile_use_the_data_protection_keychain() {
        let dir = tempfile::tempdir().unwrap();
        let contents = dir.path().join("Kubyl.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        let exe = contents.join("MacOS/kubyl");
        std::fs::write(&exe, "").unwrap();
        assert!(!keychain::profiled(&exe));
        std::fs::write(contents.join("embedded.provisionprofile"), "").unwrap();
        assert!(keychain::profiled(&exe));
        // `cargo run` / `cargo test` binaries in target/.
        assert!(!keychain::protected());
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn env_selects_the_built_in_stores() {
        assert!(matches!(from_env(Some("memory")), Backend::Memory(_)));
        assert!(matches!(from_env(Some("keychain")), Backend::Keyring));
        assert!(matches!(from_env(None), Backend::Keyring));
    }

    #[test]
    fn built_in_stores_keep_reading_writing_and_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let file = Backend::File(dir.path().join("dev-credentials.json"));
        for backend in [Backend::Memory(Mutex::default()), file] {
            let secret = SecretString::from("s3cret");
            assert!(get_from(&backend, "a").unwrap().is_none());
            set_in(&backend, "a", &secret).unwrap();
            assert_eq!(
                get_from(&backend, "a").unwrap().unwrap().expose_secret(),
                "s3cret"
            );
            delete_in(&backend, "a").unwrap();
            delete_in(&backend, "a").unwrap();
            assert!(get_from(&backend, "a").unwrap().is_none());
        }
    }

    #[test]
    fn concurrent_file_writes_keep_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dev-credentials.json");
        let threads: Vec<_> = (0..16)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    update_file(&path, |map| {
                        map.insert(format!("key{i}"), format!("value{i}"));
                        true
                    })
                    .unwrap();
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(read_file(&path).len(), 16);
        update_file(&path, |map| map.remove("key3").is_some()).unwrap();
        assert!(!read_file(&path).contains_key("key3"));
        assert_eq!(read_file(&path).len(), 15);
    }
}
