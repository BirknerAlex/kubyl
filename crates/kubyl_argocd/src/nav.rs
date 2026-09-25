//! Keyboard navigation shared by the Argo CD tables (applications, history, the resource tree,
//! ApplicationSets, Projects): `j`/`k`, arrows, Home/End, page keys, Enter.

use gpui::{App, KeyBinding, actions};

actions!(
    argocd_table,
    [
        SelectNext,
        SelectPrevious,
        SelectFirst,
        SelectLast,
        SelectPageDown,
        SelectPageUp,
        Confirm,
        /// Collapses the selected tree node (or goes to its parent).
        CollapseNode,
        /// Expands the selected tree node.
        ExpandNode,
    ]
);

/// Key context of the tables.
pub const TABLE: &str = "ArgoTable";
const PAGE: isize = 20;

pub(crate) fn init(cx: &mut App) {
    let table = Some(TABLE);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, table),
        KeyBinding::new("j", SelectNext, table),
        KeyBinding::new("up", SelectPrevious, table),
        KeyBinding::new("k", SelectPrevious, table),
        KeyBinding::new("home", SelectFirst, table),
        KeyBinding::new("g g", SelectFirst, table),
        KeyBinding::new("end", SelectLast, table),
        KeyBinding::new("shift-g", SelectLast, table),
        KeyBinding::new("pagedown", SelectPageDown, table),
        KeyBinding::new("pageup", SelectPageUp, table),
        KeyBinding::new("enter", Confirm, table),
        KeyBinding::new("left", CollapseNode, Some("ArgoTree")),
        KeyBinding::new("right", ExpandNode, Some("ArgoTree")),
    ]);
}

/// A move within `len` rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Next,
    Previous,
    First,
    Last,
    PageDown,
    PageUp,
}

/// The row after `move` from `current` (clamped; the first row when nothing is selected).
pub fn step(current: Option<usize>, len: usize, movement: Move) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let last = len as isize - 1;
    let next = match (current, movement) {
        (_, Move::First) => 0,
        (_, Move::Last) => last,
        (None, _) => 0,
        (Some(i), Move::Next) => i as isize + 1,
        (Some(i), Move::Previous) => i as isize - 1,
        (Some(i), Move::PageDown) => i as isize + PAGE,
        (Some(i), Move::PageUp) => i as isize - PAGE,
    };
    Some(next.clamp(0, last) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_clamped() {
        assert_eq!(step(None, 0, Move::Next), None);
        assert_eq!(step(None, 5, Move::Next), Some(0));
        assert_eq!(step(Some(4), 5, Move::Next), Some(4));
        assert_eq!(step(Some(0), 5, Move::Previous), Some(0));
        assert_eq!(step(Some(1), 50, Move::PageDown), Some(21));
        assert_eq!(step(Some(1), 5, Move::Last), Some(4));
    }
}
