//! Recently used palette entries (`state.json` → `palette`), for the recency boost and the
//! "Recent" group of an empty palette. Keys look like `kind:apps/deployments`,
//! `action:Resource: Delete…`, `ctx:<cluster id>`, `ns:kube-system`. No secrets are stored.

use gpui::App;
use kubyl_settings::{State, StateSection};
use serde::{Deserialize, Serialize};

/// How many keys are remembered.
pub const LIMIT: usize = 50;
/// Boost of the most recent entry; older entries get proportionally less.
pub const MAX_BOOST: u32 = 60;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recent {
    /// Most recent first.
    pub keys: Vec<String>,
}

impl StateSection for Recent {
    const KEY: &'static str = "palette";
}

impl Recent {
    pub fn load(cx: &App) -> Self {
        State::get::<Self>(cx)
    }

    /// Moves `key` to the front and saves.
    pub fn touch(cx: &mut App, key: &str) {
        State::update::<Self>(cx, |recent| recent.push(key));
    }

    fn push(&mut self, key: &str) {
        self.keys.retain(|k| k != key);
        self.keys.insert(0, key.to_string());
        self.keys.truncate(LIMIT);
    }

    /// Recency boost for `key`: `MAX_BOOST` for the latest, 0 when unknown.
    pub fn boost(&self, key: &str) -> u32 {
        self.rank(key)
            .map_or(0, |ix| MAX_BOOST * (LIMIT - ix) as u32 / LIMIT as u32)
    }

    /// Position in the list (0 = most recent).
    pub fn rank(&self, key: &str) -> Option<usize> {
        self.keys.iter().position(|k| k == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_recent_gets_the_biggest_boost() {
        let mut recent = Recent::default();
        recent.push("a");
        recent.push("b");
        recent.push("a");
        assert_eq!(recent.keys, ["a", "b"]);
        assert_eq!(recent.boost("a"), MAX_BOOST);
        assert!(recent.boost("b") < MAX_BOOST);
        assert_eq!(recent.boost("c"), 0);
        for i in 0..100 {
            recent.push(&i.to_string());
        }
        assert_eq!(recent.keys.len(), LIMIT);
    }
}
