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

    /// Moves a folder one slot up (`-1`) or down (`1`).
    pub fn move_group(&mut self, id: &str, offset: isize, cx: &mut Context<Self>) {
        if move_group_by(&mut self.state.groups, id, offset) {
            self.changed(cx);
        }
    }

    /// Drag and drop of a folder onto another: `id` takes the place of `target`.
    pub fn move_group_onto(&mut self, id: &str, target: &str, cx: &mut Context<Self>) {
        if move_group_onto(&mut self.state.groups, id, target) {
            self.changed(cx);
        }
    }

    /// Moves a member one slot up (`-1`) or down (`1`) among the members of its folder that
    /// are `visible` (not hidden by a filter).
    pub fn move_member(
        &mut self,
        cluster: &str,
        offset: isize,
        visible: &[String],
        cx: &mut Context<Self>,
    ) {
        if move_member_by(&mut self.state.groups, cluster, offset, visible) {
            self.changed(cx);
        }
    }

    /// Back to the configured sort order for the clusters outside folders.
    pub fn reset_root_order(&mut self, cx: &mut Context<Self>) {
        if !self.state.order.is_empty() {
            self.state.order.clear();
            self.changed(cx);
        }
    }

    /// Drag and drop of a cluster onto a member of a folder: it takes the member's place (in
    /// that folder, from wherever it came).
    pub fn move_member_onto(&mut self, cluster: &str, target: &str, cx: &mut Context<Self>) {
        if move_member_onto(&mut self.state.groups, cluster, target) {
            self.changed(cx);
        }
    }

    /// Whether the folder can move up (`-1`) or down (`1`).
    pub fn can_move_group(&self, id: &str, offset: isize) -> bool {
        let at = self.state.groups.iter().position(|g| g.id == id);
        at.and_then(|at| at.checked_add_signed(offset))
            .is_some_and(|to| to < self.state.groups.len())
    }

    /// The manual order of the clusters outside folders.
    pub fn root_order(&self) -> &[String] {
        &self.state.order
    }

    /// Drag and drop of a cluster onto one outside the folders: it leaves its folder (if any)
    /// and takes the place of `target` in `visible` (the clusters outside folders, as shown).
    pub fn move_root_onto(
        &mut self,
        visible: &[String],
        cluster: &str,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        let ungrouped = remove_member(&mut self.state.groups, cluster);
        if move_root_onto(&mut self.state.order, visible, cluster, target) || ungrouped {
            self.changed(cx);
        }
    }

    /// Moves a cluster outside the folders one slot up (`-1`) or down (`1`) in `visible`.
    pub fn move_root(
        &mut self,
        visible: &[String],
        cluster: &str,
        offset: isize,
        cx: &mut Context<Self>,
    ) {
        if move_root_by(&mut self.state.order, visible, cluster, offset) {
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
        let resolve = |member: &str| {
            manager
                .resolve(&ClusterId::new(member.to_string()))
                .to_string()
        };
        let changed = resolve_members(&mut self.state.groups, resolve);
        let order_changed = resolve_order(&mut self.state.order, resolve);
        if changed || order_changed {
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

/// Re-keys the manual order through `resolve`, dropping ids that then repeat. Returns whether
/// anything changed.
fn resolve_order(order: &mut Vec<String>, resolve: impl Fn(&str) -> String) -> bool {
    let before = order.clone();
    let mut seen = std::collections::HashSet::new();
    *order = before
        .iter()
        .map(|id| resolve(id))
        .filter(|id| seen.insert(id.clone()))
        .collect();
    *order != before
}

/// Sorts `clusters` by their place in the manual `order`; unlisted ones keep their relative
/// order after the listed ones.
pub fn sort_by_order<T>(order: &[String], clusters: &mut [T], id: impl Fn(&T) -> &str) {
    clusters.sort_by_key(|c| order.iter().position(|o| o == id(c)).unwrap_or(usize::MAX));
}

/// The new manual order after moving `cluster` inside `visible` (the clusters outside folders,
/// as shown): the moved list first, then entries of `order` that aren't visible (hidden by a
/// filter) so they keep their place when they come back.
fn apply_root_order(order: &mut Vec<String>, moved: Vec<String>) {
    let rest: Vec<String> = order
        .iter()
        .filter(|o| !moved.contains(o))
        .cloned()
        .collect();
    *order = moved;
    order.extend(rest);
}

/// `cluster` takes the place of `target` in `visible`; one coming from elsewhere (a folder) goes
/// in front of it, like [`move_member_onto`]. Returns whether anything changed.
fn move_root_onto(
    order: &mut Vec<String>,
    visible: &[String],
    cluster: &str,
    target: &str,
) -> bool {
    if cluster == target || !visible.iter().any(|v| v == target) {
        return false;
    }
    let mut moved: Vec<String> = visible.to_vec();
    let from = moved.iter().position(|v| v == cluster);
    if let Some(from) = from {
        moved.remove(from);
    }
    let mut to = moved.iter().position(|v| v == target).unwrap_or(0);
    // Dragged down: below the target.
    if from.is_some_and(|from| from <= to) {
        to += 1;
    }
    moved.insert(to, cluster.to_string());
    if moved == visible && order.starts_with(&moved) {
        return false;
    }
    apply_root_order(order, moved);
    true
}

/// Moves `cluster` by `offset` slots in `visible`. Returns whether anything changed.
fn move_root_by(order: &mut Vec<String>, visible: &[String], cluster: &str, offset: isize) -> bool {
    let Some(from) = visible.iter().position(|v| v == cluster) else {
        return false;
    };
    let Some(to) = from
        .checked_add_signed(offset)
        .filter(|&to| to < visible.len())
    else {
        return false;
    };
    let mut moved = visible.to_vec();
    let id = moved.remove(from);
    moved.insert(to, id);
    apply_root_order(order, moved);
    true
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

/// Moves the folder `id` by `offset` slots. Returns whether anything changed.
fn move_group_by(groups: &mut Vec<SidebarGroup>, id: &str, offset: isize) -> bool {
    let Some(from) = groups.iter().position(|g| g.id == id) else {
        return false;
    };
    let Some(to) = from
        .checked_add_signed(offset)
        .filter(|&to| to < groups.len())
    else {
        return false;
    };
    if from == to {
        return false;
    }
    let group = groups.remove(from);
    groups.insert(to, group);
    true
}

/// The folder `id` takes the position of `target`. Returns whether anything changed.
fn move_group_onto(groups: &mut Vec<SidebarGroup>, id: &str, target: &str) -> bool {
    let (Some(from), Some(to)) = (
        groups.iter().position(|g| g.id == id),
        groups.iter().position(|g| g.id == target),
    ) else {
        return false;
    };
    if from == to {
        return false;
    }
    let group = groups.remove(from);
    groups.insert(to, group);
    true
}

/// Moves `cluster` by `offset` slots among the `visible` members of its folder (it takes the
/// place of the visible neighbour). Returns whether anything changed.
fn move_member_by(
    groups: &mut [SidebarGroup],
    cluster: &str,
    offset: isize,
    visible: &[String],
) -> bool {
    let Some(group) = groups
        .iter_mut()
        .find(|g| g.members.iter().any(|m| m == cluster))
    else {
        return false;
    };
    let shown: Vec<&String> = group
        .members
        .iter()
        .filter(|m| m.as_str() == cluster || visible.contains(m))
        .collect();
    let Some(at) = shown.iter().position(|m| m.as_str() == cluster) else {
        return false;
    };
    let Some(neighbour) = at
        .checked_add_signed(offset)
        .and_then(|to| shown.get(to))
        .map(|m| m.to_string())
    else {
        return false;
    };
    let (Some(from), Some(to)) = (
        group.members.iter().position(|m| m == cluster),
        group.members.iter().position(|m| *m == neighbour),
    ) else {
        return false;
    };
    let member = group.members.remove(from);
    group.members.insert(to, member);
    true
}

/// `cluster` takes the position of `target` in the folder `target` is in. Coming from another
/// folder (or none) it goes in front of `target`; inside the folder it swaps places, so
/// dragging down lands below `target`. Returns whether anything changed.
fn move_member_onto(groups: &mut [SidebarGroup], cluster: &str, target: &str) -> bool {
    if cluster == target {
        return false;
    }
    let Some(group) = groups
        .iter()
        .find(|g| g.members.iter().any(|m| m == target))
    else {
        return false;
    };
    let group_id = group.id.clone();
    let to = group.members.iter().position(|m| m == target).unwrap_or(0);
    let from = group.members.iter().position(|m| m == cluster);
    let before = match from {
        // Dragged down: in front of whatever follows the target (the end when nothing does).
        Some(from) if from < to => group.members.get(to + 1).cloned(),
        _ => Some(target.to_string()),
    };
    add_member(groups, &group_id, cluster, before.as_deref())
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

    fn ids(groups: &[SidebarGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.id.as_str()).collect()
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
    fn move_group_by_steps_and_stops_at_the_ends() {
        let mut groups = vec![group("a", &[]), group("b", &[]), group("c", &[])];
        assert!(move_group_by(&mut groups, "b", -1));
        assert_eq!(ids(&groups), ["b", "a", "c"]);
        assert!(!move_group_by(&mut groups, "b", -1));
        assert!(move_group_by(&mut groups, "b", 1));
        assert!(move_group_by(&mut groups, "b", 1));
        assert_eq!(ids(&groups), ["a", "c", "b"]);
        assert!(!move_group_by(&mut groups, "b", 1));
        assert!(!move_group_by(&mut groups, "missing", 1));
    }

    #[test]
    fn move_group_onto_takes_the_targets_place() {
        let mut groups = vec![group("a", &[]), group("b", &[]), group("c", &[])];
        assert!(move_group_onto(&mut groups, "a", "c"));
        assert_eq!(ids(&groups), ["b", "c", "a"]);
        assert!(move_group_onto(&mut groups, "a", "b"));
        assert_eq!(ids(&groups), ["a", "b", "c"]);
        assert!(!move_group_onto(&mut groups, "a", "a"));
    }

    #[test]
    fn move_member_by_stays_inside_the_folder() {
        let mut groups = vec![group("prod", &["a", "b"]), group("staging", &["c"])];
        let all = strings(&["a", "b", "c"]);
        assert!(move_member_by(&mut groups, "a", 1, &all));
        assert_eq!(groups[0].members, vec!["b", "a"]);
        assert!(!move_member_by(&mut groups, "a", 1, &all));
        assert!(!move_member_by(&mut groups, "c", -1, &all));
        assert!(!move_member_by(&mut groups, "ungrouped", 1, &all));
    }

    #[test]
    fn move_member_by_skips_hidden_members() {
        let mut groups = vec![group("prod", &["a", "hidden", "b", "c"])];
        let visible = strings(&["a", "b", "c"]);
        assert!(move_member_by(&mut groups, "b", -1, &visible));
        assert_eq!(groups[0].members, vec!["b", "a", "hidden", "c"]);
        // Only hidden members below: nothing to swap with.
        let mut groups = vec![group("prod", &["a", "hidden"])];
        assert!(!move_member_by(&mut groups, "a", 1, &strings(&["a"])));
    }

    #[test]
    fn move_member_onto_reorders_inside_and_across_folders() {
        let mut groups = vec![group("prod", &["a", "b", "c"]), group("staging", &["d"])];
        // Dragged down: lands below the target.
        assert!(move_member_onto(&mut groups, "a", "b"));
        assert_eq!(groups[0].members, vec!["b", "a", "c"]);
        assert!(move_member_onto(&mut groups, "b", "c"));
        assert_eq!(groups[0].members, vec!["a", "c", "b"]);
        // Dragged up: takes the target's place.
        assert!(move_member_onto(&mut groups, "b", "a"));
        assert_eq!(groups[0].members, vec!["b", "a", "c"]);
        // From another folder or none: in front of the target.
        assert!(move_member_onto(&mut groups, "d", "a"));
        assert_eq!(groups[0].members, vec!["b", "d", "a", "c"]);
        assert!(groups[1].members.is_empty());
        assert!(move_member_onto(&mut groups, "new", "b"));
        assert_eq!(groups[0].members, vec!["new", "b", "d", "a", "c"]);
        assert!(!move_member_onto(&mut groups, "b", "b"));
        assert!(!move_member_onto(&mut groups, "b", "ungrouped"));
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn move_root_onto_freezes_the_shown_order_and_moves() {
        let visible = strings(&["a", "b", "c", "d"]);
        let mut order = Vec::new();
        // Dragged down: below the target.
        assert!(move_root_onto(&mut order, &visible, "a", "b"));
        assert_eq!(order, ["b", "a", "c", "d"]);
        // Dragged up: in front of the target.
        let visible = strings(&["b", "a", "c", "d"]);
        assert!(move_root_onto(&mut order, &visible, "d", "b"));
        assert_eq!(order, ["d", "b", "a", "c"]);
        assert!(!move_root_onto(&mut order, &visible, "a", "a"));
        assert!(!move_root_onto(&mut order, &visible, "a", "missing"));
    }

    #[test]
    fn move_root_onto_from_a_folder_goes_in_front_and_keeps_hidden_entries() {
        let visible = strings(&["a", "b"]);
        let mut order = strings(&["h", "a", "b"]);
        assert!(move_root_onto(&mut order, &visible, "x", "b"));
        assert_eq!(order, ["a", "x", "b", "h"]);
    }

    #[test]
    fn move_root_by_steps_inside_the_visible_list() {
        let visible = strings(&["a", "b", "c"]);
        let mut order = Vec::new();
        assert!(move_root_by(&mut order, &visible, "c", -1));
        assert_eq!(order, ["a", "c", "b"]);
        assert!(!move_root_by(&mut order, &visible, "a", -1));
        assert!(!move_root_by(&mut order, &visible, "c", 1));
    }

    #[test]
    fn sort_by_order_puts_listed_clusters_first() {
        let order = strings(&["c", "a"]);
        let mut clusters = strings(&["a", "b", "c", "d"]);
        sort_by_order(&order, &mut clusters, |c| c.as_str());
        assert_eq!(clusters, ["c", "a", "b", "d"]);
    }

    #[test]
    fn resolve_order_rekeys_and_dedupes() {
        let mut order = strings(&["a", "b", "c"]);
        let resolved = |m: &str| {
            if m == "b" {
                "a".to_string()
            } else {
                m.to_string()
            }
        };
        assert!(resolve_order(&mut order, resolved));
        assert_eq!(order, ["a", "c"]);
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
