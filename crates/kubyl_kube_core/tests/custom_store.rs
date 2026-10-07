//! An installed credential store replaces the built-in ones. The choice is process-wide, so this
//! file has a single test.

use std::collections::BTreeMap;
use std::sync::Arc;

use kubyl_kube_core::auth::store::{self, Credentials, SecretStore};
use parking_lot::Mutex;
use secrecy::{ExposeSecret as _, SecretString};

#[derive(Default)]
struct Custom(Arc<Mutex<BTreeMap<String, String>>>);

impl SecretStore for Custom {
    fn name(&self) -> &'static str {
        "custom test store"
    }

    fn get(&self, key: &str) -> Result<Option<SecretString>, String> {
        Ok(self.0.lock().get(key).cloned().map(SecretString::from))
    }

    fn set(&self, key: &str, secret: &SecretString) -> Result<(), String> {
        self.0
            .lock()
            .insert(key.to_string(), secret.expose_secret().to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().remove(key);
        Ok(())
    }
}

fn scopes_keep_their_secrets_apart(entries: &Mutex<BTreeMap<String, String>>) {
    let alice = Credentials::scoped("alice");
    let bob = Credentials::scoped("bob");
    let desktop = Credentials::default();
    let key = "oidc/https://idp.example/kubyl";

    alice
        .set(key, &SecretString::from("alice-refresh"))
        .unwrap();
    assert_eq!(
        alice.get(key).unwrap().unwrap().expose_secret(),
        "alice-refresh"
    );
    assert!(bob.get(key).unwrap().is_none());
    assert!(desktop.get(key).unwrap().is_none());

    bob.set(key, &SecretString::from("bob-refresh")).unwrap();
    desktop.set(key, &SecretString::from("desktop")).unwrap();
    // The default handle keeps its keys as they were; scopes are prefixed.
    assert_eq!(entries.lock().get(key).map(String::as_str), Some("desktop"));
    assert!(
        entries
            .lock()
            .contains_key("scope/alice/oidc/https://idp.example/kubyl")
    );

    // A `/` in a scope can't make two scopes meet.
    assert_ne!(
        Credentials::scoped("a").key("b/c"),
        Credentials::scoped("a/b").key("c")
    );
    assert_ne!(
        Credentials::scoped("a%2Fb").key("c"),
        Credentials::scoped("a/b").key("c")
    );
    assert_eq!(Credentials::default().key("x/y"), "x/y");

    // Deleting in one scope leaves the others.
    alice.delete(key).unwrap();
    assert!(alice.get(key).unwrap().is_none());
    assert_eq!(
        bob.get(key).unwrap().unwrap().expose_secret(),
        "bob-refresh"
    );
    bob.delete(key).unwrap();
    desktop.delete(key).unwrap();
}

#[test]
fn installed_store_serves_every_call() {
    let entries = Arc::new(Mutex::new(BTreeMap::new()));
    store::install(Box::new(Custom(entries.clone()))).unwrap();
    assert_eq!(store::store_name(), "custom test store");

    let credentials = Credentials::default();
    assert!(credentials.get("cluster/refresh").unwrap().is_none());
    credentials
        .set("cluster/refresh", &SecretString::from("r3fresh"))
        .unwrap();
    assert_eq!(
        credentials
            .get("cluster/refresh")
            .unwrap()
            .unwrap()
            .expose_secret(),
        "r3fresh"
    );
    assert_eq!(entries.lock().len(), 1);
    credentials.delete("cluster/refresh").unwrap();
    credentials.delete("cluster/refresh").unwrap();
    assert!(entries.lock().is_empty());

    scopes_keep_their_secrets_apart(&entries);
    assert!(entries.lock().is_empty());

    // The store can't change once chosen.
    assert!(store::install(Box::new(Custom::default())).is_err());
    assert_eq!(store::store_name(), "custom test store");
}
