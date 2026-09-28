//! User-created sidebar folders (e.g. "Prod", "Staging") that group cluster roots across
//! kubeconfigs. Mirrors [`crate::favorites::Favorites`]: a global entity backed by state.json,
//! so `ClustersSection` (and anything else) can observe changes.

use gpui::{App, AppContext as _, Context, Entity, Global};
use kubyl_core::ClusterId;
use kubyl_kube::{ConnectionEvent, ConnectionManager};
use kubyl_settings::State;

use crate::settings::{SidebarGroup, SidebarGroupsState};

/// The app's sidebar folders. Views observe [`SidebarGroups::global`].
pub struct SidebarGroups {
    state: SidebarGroupsState,
    _subscription: Option<gpui::Subscription>,
}

struct GlobalSidebarGroups(Entity<SidebarGroups>);

impl Global for GlobalSidebarGroups {}

impl SidebarGroups {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSidebarGroups>().0.clone()
    }

    pub(crate) fn install(cx: &mut App) {
        let state = State::get::<SidebarGroupsState>(cx);
        let entity = cx.new(|cx| {
            let subscription = ConnectionManager::try_global(cx).map(|manager| {
                cx.subscribe(
                    &manager,
                    |this: &mut Self, _, event: &ConnectionEvent, cx| {
                        if matches!(
                            event,
                            ConnectionEvent::ContextsChanged | ConnectionEvent::Rekeyed { .. }
                        ) {
                            this.migrate_ids(cx);
                        }
                    },
                )
            });
            Self {
                state,
                _subscription: subscription,
            }
        });
        cx.set_global(GlobalSidebarGroups(entity));
    }

    pub fn groups(&self) -> &[SidebarGroup] {
        &self.state.groups
    }

    pub fn is_collapsed(&self, id: &str) -> bool {
        self.state.collapsed.contains(id)
    }

    /// The group `cluster` currently belongs to, if any.
    pub fn group_of(&self, cluster: &str) -> Option<&SidebarGroup> {
        self.state
            .groups
            .iter()
            .find(|g| g.members.iter().any(|m| m == cluster))
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        State::set(cx, &self.state);
        cx.notify();
    }

    /// Creates an empty folder named `name`. Returns its id.
    pub fn create(&mut self, name: String, cx: &mut Context<Self>) -> String {
        let id = new_id();
        self.state.groups.push(SidebarGroup {
            id: id.clone(),
            name,
            members: Vec::new(),
        });
        self.changed(cx);
        id
    }

    pub fn rename(&mut self, id: &str, name: String, cx: &mut Context<Self>) {
        if let Some(group) = self.state.groups.iter_mut().find(|g| g.id == id) {
            group.name = name;
            self.changed(cx);
        }
    }

    /// Deletes the folder; its members simply become ungrouped again.
    pub fn delete(&mut self, id: &str, cx: &mut Context<Self>) {
        let before = self.state.groups.len();
        self.state.groups.retain(|g| g.id != id);
        if self.state.groups.len() != before {
            self.state.collapsed.remove(id);
            self.changed(cx);
        }
    }

    pub fn toggle_collapsed(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.state.collapsed.remove(id) {
            self.state.collapsed.insert(id.to_string());
        }
        self.changed(cx);
    }

    /// Removes `cluster` from every group (ungroups it).
    pub fn remove_member(&mut self, cluster: &str, cx: &mut Context<Self>) {
        if remove_member(&mut self.state.groups, cluster) {
            self.changed(cx);
        }
    }

    /// Moves `cluster` into `group_id`, removing it from any other group first. Appends unless
    /// `before` names another member to insert ahead of (drag-and-drop reordering).
    pub fn add_member(
        &mut self,
        group_id: &str,
        cluster: &str,
        before: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if add_member(&mut self.state.groups, group_id, cluster, before) {
            self.changed(cx);
        }
    }

    /// Keeps group members under the current cluster ids (contexts were grouped, an entry was
    /// re-keyed), like `ClustersSection::migrate_ids`.
    fn migrate_ids(&mut self, cx: &mut Context<Self>) {
        let Some(manager) = ConnectionManager::try_global(cx) else {
            return;
        };
        let manager = manager.read(cx);
        if manager.is_loading() {
            return;
        }
        let changed = resolve_members(&mut self.state.groups, |member| {
            manager
                .resolve(&ClusterId::new(member.to_string()))
                .to_string()
        });
        if changed {
            self.changed(cx);
        }
    }
}

/// Re-keys every member through `resolve` and drops any id that then duplicates one already
/// seen (two contexts, one in each of two groups, can resolve to the same cluster once they're
/// grouped into a single entry: the cluster keeps its first group). Returns whether anything
/// changed.
fn resolve_members(groups: &mut [SidebarGroup], resolve: impl Fn(&str) -> String) -> bool {
    let mut changed = false;
    let mut seen = std::collections::HashSet::new();
    for group in groups.iter_mut() {
        for member in &mut group.members {
            let resolved = resolve(member);
            if &resolved != member {
                *member = resolved;
                changed = true;
            }
        }
        let before = group.members.len();
        group.members.retain(|m| seen.insert(m.clone()));
        changed |= group.members.len() != before;
    }
    changed
}

/// Removes `cluster` from every group. Returns whether anything changed.
fn remove_member(groups: &mut [SidebarGroup], cluster: &str) -> bool {
    let mut changed = false;
    for group in groups {
        let before = group.members.len();
        group.members.retain(|m| m != cluster);
        changed |= group.members.len() != before;
    }
    changed
}

/// Moves `cluster` into `group_id`, removing it from any other group first. Appends unless
/// `before` names another member to insert ahead of. Returns whether anything changed (`false`
/// if `group_id` doesn't exist).
fn add_member(
    groups: &mut [SidebarGroup],
    group_id: &str,
    cluster: &str,
    before: Option<&str>,
) -> bool {
    if !groups.iter().any(|g| g.id == group_id) {
        return false;
    }
    remove_member(groups, cluster);
    let Some(group) = groups.iter_mut().find(|g| g.id == group_id) else {
        return false;
    };
    let index = before
        .and_then(|b| group.members.iter().position(|m| m == b))
        .unwrap_or(group.members.len());
    group.members.insert(index, cluster.to_string());
    true
}

/// A short, unique-enough id for a new folder (never shown to the user).
fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("group-{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, members: &[&str]) -> SidebarGroup {
        SidebarGroup {
            id: id.into(),
            name: id.into(),
            members: members.iter().map(|m| m.to_string()).collect(),
        }
    }

    #[test]
    fn add_member_moves_between_groups() {
        let mut groups = vec![group("prod", &["a", "b"]), group("staging", &["c"])];
        assert!(add_member(&mut groups, "staging", "a", None));
        assert_eq!(groups[0].members, vec!["b"]);
        assert_eq!(groups[1].members, vec!["c", "a"]);
    }

    #[test]
    fn add_member_inserts_before_another() {
        let mut groups = vec![group("prod", &["a", "b"])];
        assert!(add_member(&mut groups, "prod", "c", Some("b")));
        assert_eq!(groups[0].members, vec!["a", "c", "b"]);
    }

    #[test]
    fn add_member_to_missing_group_is_a_no_op() {
        let mut groups = vec![group("prod", &["a"])];
        assert!(!add_member(&mut groups, "missing", "a", None));
        assert_eq!(groups[0].members, vec!["a"]);
    }

    #[test]
    fn remove_member_ungroups_a_cluster() {
        let mut groups = vec![group("prod", &["a", "b"])];
        assert!(remove_member(&mut groups, "a"));
        assert_eq!(groups[0].members, vec!["b"]);
        assert!(!remove_member(&mut groups, "a"));
    }

    #[test]
    fn resolve_members_dedupes_within_a_group() {
        // "a" and "b" both resolve to "merged" once their contexts are grouped into one entry.
        let mut groups = vec![group("prod", &["a", "b", "c"])];
        let resolved = |m: &str| {
            if m == "b" {
                "a".to_string()
            } else {
                m.to_string()
            }
        };
        assert!(resolve_members(&mut groups, resolved));
        assert_eq!(groups[0].members, vec!["a", "c"]);
    }

    #[test]
    fn resolve_members_dedupes_across_groups_keeping_the_first() {
        let mut groups = vec![group("prod", &["a"]), group("staging", &["b"])];
        let resolved = |m: &str| {
            if m == "b" {
                "a".to_string()
            } else {
                m.to_string()
            }
        };
        assert!(resolve_members(&mut groups, resolved));
        assert_eq!(groups[0].members, vec!["a"]);
        assert!(groups[1].members.is_empty());
    }

    #[test]
    fn resolve_members_is_a_no_op_when_nothing_resolves_differently() {
        let mut groups = vec![group("prod", &["a", "b"])];
        assert!(!resolve_members(&mut groups, |m| m.to_string()));
        assert_eq!(groups[0].members, vec!["a", "b"]);
    }
}
