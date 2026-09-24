//! The split tree of the center area.

use gpui::{AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString};
use gpui_component::resizable::{h_resizable, resizable_panel, v_resizable};

use super::layout::{PaneLayout, SplitAxis};
use super::pane::Pane;

pub enum Member {
    Pane(Entity<Pane>),
    Split {
        id: usize,
        axis: SplitAxis,
        members: Vec<Member>,
    },
}

/// Panes arranged in nested horizontal/vertical splits.
pub struct PaneGroup {
    root: Member,
    next_id: usize,
}

impl PaneGroup {
    /// Builds a group from a persisted layout. `make_pane` creates a pane for each leaf.
    pub fn from_layout(
        layout: &PaneLayout,
        make_pane: &mut dyn FnMut(&[kubyl_core::ViewRequest], usize) -> Entity<Pane>,
    ) -> Self {
        let mut next_id = 0;
        let root = Self::member_from_layout(layout, make_pane, &mut next_id);
        Self { root, next_id }
    }

    fn member_from_layout(
        layout: &PaneLayout,
        make_pane: &mut dyn FnMut(&[kubyl_core::ViewRequest], usize) -> Entity<Pane>,
        next_id: &mut usize,
    ) -> Member {
        match layout {
            PaneLayout::Pane { tabs, active } => Member::Pane(make_pane(tabs, *active)),
            PaneLayout::Split { children, .. } if children.is_empty() => {
                Member::Pane(make_pane(&[], 0))
            }
            PaneLayout::Split { children, .. } if children.len() == 1 => {
                Self::member_from_layout(&children[0], make_pane, next_id)
            }
            PaneLayout::Split { axis, children } => {
                *next_id += 1;
                let id = *next_id;
                Member::Split {
                    id,
                    axis: *axis,
                    members: children
                        .iter()
                        .map(|child| Self::member_from_layout(child, make_pane, next_id))
                        .collect(),
                }
            }
        }
    }

    /// All panes, in visual order.
    pub fn panes(&self) -> Vec<Entity<Pane>> {
        fn collect(member: &Member, out: &mut Vec<Entity<Pane>>) {
            match member {
                Member::Pane(pane) => out.push(pane.clone()),
                Member::Split { members, .. } => members.iter().for_each(|m| collect(m, out)),
            }
        }
        let mut out = Vec::new();
        collect(&self.root, &mut out);
        out
    }

    /// Puts `new` next to `target`. Returns false if `target` isn't in the group.
    pub fn split(&mut self, target: &Entity<Pane>, new: Entity<Pane>, axis: SplitAxis) -> bool {
        let mut new = Some(new);
        let mut next_id = self.next_id;
        let found = Self::split_member(&mut self.root, target, &mut new, axis, &mut next_id);
        self.next_id = next_id;
        found
    }

    fn split_member(
        member: &mut Member,
        target: &Entity<Pane>,
        new: &mut Option<Entity<Pane>>,
        axis: SplitAxis,
        next_id: &mut usize,
    ) -> bool {
        match member {
            Member::Pane(pane) if pane == target => {
                *next_id += 1;
                let old = std::mem::replace(member, Member::Pane(target.clone()));
                *member = Member::Split {
                    id: *next_id,
                    axis,
                    members: vec![old, Member::Pane(new.take().expect("split once"))],
                };
                true
            }
            Member::Pane(_) => false,
            Member::Split {
                axis: split_axis,
                members,
                ..
            } => {
                // Same direction: insert as a sibling instead of nesting.
                if *split_axis == axis
                    && let Some(index) = members
                        .iter()
                        .position(|m| matches!(m, Member::Pane(p) if p == target))
                {
                    members.insert(index + 1, Member::Pane(new.take().expect("split once")));
                    return true;
                }
                members
                    .iter_mut()
                    .any(|m| Self::split_member(m, target, new, axis, next_id))
            }
        }
    }

    /// Removes `pane`. The last pane is never removed; returns false then.
    pub fn remove(&mut self, pane: &Entity<Pane>) -> bool {
        if matches!(&self.root, Member::Pane(_)) {
            return false;
        }
        let removed = Self::remove_member(&mut self.root, pane);
        Self::collapse(&mut self.root);
        removed
    }

    fn remove_member(member: &mut Member, pane: &Entity<Pane>) -> bool {
        let Member::Split { members, .. } = member else {
            return false;
        };
        if let Some(index) = members
            .iter()
            .position(|m| matches!(m, Member::Pane(p) if p == pane))
        {
            members.remove(index);
            return true;
        }
        members.iter_mut().any(|m| Self::remove_member(m, pane))
    }

    /// Replaces splits that have a single child with that child.
    fn collapse(member: &mut Member) {
        if let Member::Split { members, .. } = member {
            members.iter_mut().for_each(Self::collapse);
            if members.len() == 1 {
                let only = members.pop().expect("one member");
                *member = only;
            }
        }
    }

    pub fn layout(&self, cx: &App) -> PaneLayout {
        fn to_layout(member: &Member, cx: &App) -> PaneLayout {
            match member {
                Member::Pane(pane) => pane.read(cx).layout(cx),
                Member::Split { axis, members, .. } => PaneLayout::Split {
                    axis: *axis,
                    children: members.iter().map(|m| to_layout(m, cx)).collect(),
                },
            }
        }
        to_layout(&self.root, cx)
    }

    pub fn render(&self) -> AnyElement {
        Self::render_member(&self.root)
    }

    fn render_member(member: &Member) -> AnyElement {
        match member {
            Member::Pane(pane) => pane.clone().into_any_element(),
            Member::Split { id, axis, members } => {
                let id = SharedString::from(format!("split-{id}"));
                let group = match axis {
                    SplitAxis::Horizontal => h_resizable(id),
                    SplitAxis::Vertical => v_resizable(id),
                };
                group
                    .children(
                        members
                            .iter()
                            .map(|m| resizable_panel().child(Self::render_member(m))),
                    )
                    .into_any_element()
            }
        }
    }
}
