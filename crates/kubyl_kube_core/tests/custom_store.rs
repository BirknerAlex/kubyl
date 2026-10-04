//! An installed credential store replaces the built-in ones. The choice is process-wide, so this
//! file has a single test.

use std::collections::BTreeMap;
use std::sync::Arc;

use kubyl_kube_core::auth::store::{self, SecretStore};
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

#[test]
fn installed_store_serves_every_call() {
    let entries = Arc::new(Mutex::new(BTreeMap::new()));
    store::install(Box::new(Custom(entries.clone()))).unwrap();
    assert_eq!(store::store_name(), "custom test store");

    assert!(store::get("cluster/refresh").unwrap().is_none());
    store::set("cluster/refresh", &SecretString::from("r3fresh")).unwrap();
    assert_eq!(
        store::get("cluster/refresh")
            .unwrap()
            .unwrap()
            .expose_secret(),
        "r3fresh"
    );
    assert_eq!(entries.lock().len(), 1);
    store::delete("cluster/refresh").unwrap();
    store::delete("cluster/refresh").unwrap();
    assert!(entries.lock().is_empty());

    // The store can't change once chosen.
    assert!(store::install(Box::new(Custom::default())).is_err());
    assert_eq!(store::store_name(), "custom test store");
}
