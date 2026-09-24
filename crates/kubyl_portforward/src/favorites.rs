//! Persisted favorite forwards (`state.json`, `"port_forwards"`), optionally auto-started when
//! their cluster connects.

use kubyl_settings::StateSection;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FavoriteForward {
    /// The context/cluster id this favorite belongs to.
    pub cluster: String,
    pub namespace: String,
    pub kind: String,
    pub name: String,
    /// Container port (Pod/workload forwards) or Service port (Service forwards).
    pub port: u16,
    pub local_port: u16,
    pub bind_address: String,
    pub auto_start: bool,
}

impl Default for FavoriteForward {
    fn default() -> Self {
        Self {
            cluster: String::new(),
            namespace: String::new(),
            kind: String::new(),
            name: String::new(),
            port: 0,
            local_port: 0,
            bind_address: "127.0.0.1".into(),
            auto_start: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Favorites {
    pub forwards: Vec<FavoriteForward>,
}

impl StateSection for Favorites {
    const KEY: &'static str = "port_forwards";
}

impl Favorites {
    /// Adds or replaces a favorite with the same (cluster, namespace, kind, name, port).
    pub fn upsert(&mut self, favorite: FavoriteForward) {
        self.forwards.retain(|f| !same_target(f, &favorite));
        self.forwards.push(favorite);
    }

    pub fn remove(&mut self, cluster: &str, namespace: &str, kind: &str, name: &str, port: u16) {
        self.forwards.retain(|f| {
            !(f.cluster == cluster
                && f.namespace == namespace
                && f.kind == kind
                && f.name == name
                && f.port == port)
        });
    }

    pub fn auto_start(&self, cluster: &str) -> impl Iterator<Item = &FavoriteForward> {
        self.forwards
            .iter()
            .filter(move |f| f.cluster == cluster && f.auto_start)
    }
}

fn same_target(a: &FavoriteForward, b: &FavoriteForward) -> bool {
    a.cluster == b.cluster
        && a.namespace == b.namespace
        && a.kind == b.kind
        && a.name == b.name
        && a.port == b.port
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forward(name: &str, port: u16) -> FavoriteForward {
        FavoriteForward {
            cluster: "dev".into(),
            namespace: "default".into(),
            kind: "service".into(),
            name: name.into(),
            port,
            local_port: 8080,
            bind_address: "127.0.0.1".into(),
            auto_start: false,
        }
    }

    #[test]
    fn upsert_replaces_the_same_target() {
        let mut favs = Favorites::default();
        favs.upsert(forward("web", 80));
        let mut replacement = forward("web", 80);
        replacement.local_port = 9090;
        favs.upsert(replacement);
        assert_eq!(favs.forwards.len(), 1);
        assert_eq!(favs.forwards[0].local_port, 9090);
    }

    #[test]
    fn remove_drops_only_the_matching_target() {
        let mut favs = Favorites::default();
        favs.upsert(forward("web", 80));
        favs.upsert(forward("api", 443));
        favs.remove("dev", "default", "service", "web", 80);
        assert_eq!(favs.forwards.len(), 1);
        assert_eq!(favs.forwards[0].name, "api");
    }

    #[test]
    fn auto_start_filters_by_cluster_and_flag() {
        let mut favs = Favorites::default();
        let mut auto = forward("web", 80);
        auto.auto_start = true;
        favs.upsert(auto);
        favs.upsert(forward("api", 443));
        let names: Vec<_> = favs.auto_start("dev").map(|f| f.name.clone()).collect();
        assert_eq!(names, ["web"]);
    }
}
