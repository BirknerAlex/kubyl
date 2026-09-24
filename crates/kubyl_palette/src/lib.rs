//! Command palette (board 6 · Palette).
//!
//! `⌘K` / `Ctrl+K` (and the title bar's search) open it; `:` in a list opens it in resource
//! mode. The first character picks the mode:
//!
//! - `:` resource kinds of the active cluster (names, short names, singular, `kind.group`,
//!   categories such as `all`, CRDs included) with live counts, plus k9s commands:
//!   `:pods payments`, `:deploy -A`, `:ctx staging`, `:ns kube-system`, `:q`;
//! - `@` contexts, `#` namespaces, `*` favorites (and the Favorites workspace per kind);
//! - `>` actions from the [`ActionRegistry`] that apply where the palette was opened (the
//!   selection, the focused key context), with the keys that run them there;
//! - `/` filter the focused list.
//!
//! Without a prefix it searches all of them (and loaded objects). `⌘P` is "Go to object" and
//! `shift-o` in a list lists the selection's references (owner, node, secrets, selected pods…).
//!
//! Results are fuzzy-matched (`nucleo-matcher`), grouped, highlighted and boosted by recency
//! (`state.json` → `palette`). `↵` opens, `⌘↵` opens in a split, `⇥` toggles all namespaces.

mod command;
mod items;
mod matcher;
mod palette;
mod recent;
/// What an object refers to; also used by the YAML editor's related-objects list.
pub mod references;

use gpui::{
    Action, App, AppContext as _, Global, KeyBinding, KeyContext, Keymap, ParentElement as _,
    Styled as _, WeakEntity, Window, actions,
};
use gpui_component::WindowExt as _;
use kubyl_core::actions::ToggleCommandPalette;
use kubyl_core::{ActionRegistry, ActionSpec, ActiveContext, Notification, NotificationCenter};
use kubyl_resources::ResourceSelection;
use kubyl_ui::ActiveColors;
use schemars::JsonSchema;
use serde::Deserialize;

pub use command::Mode;
use palette::{CONTEXT, CommandPalette, EMPTY_CONTEXT, Origin};

/// Opens the palette in a mode, optionally with a query (`{"query": ":cert"}` picks the mode
/// from the prefix). Bindable from keymaps: `["palette::Open", {"mode": "resources"}]`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = palette)]
#[serde(default, deny_unknown_fields)]
pub struct Open {
    pub mode: Option<Mode>,
    pub query: Option<String>,
}

impl Open {
    pub fn mode(mode: Mode) -> Self {
        Self {
            mode: Some(mode),
            query: None,
        }
    }
}

actions!(
    palette,
    [
        /// Quick open by name across the loaded caches of all connected clusters.
        GoToObject,
        /// Lists what the selected object refers to (owner, node, secrets, selected pods…).
        GoToReference,
        /// Shows all namespaces (k9s `0`).
        ShowAllNamespaces,
        /// Adds the active namespace to the favorites.
        AddNamespaceToFavorites,
        SelectNext,
        SelectPrevious,
        /// `↵`: runs the selected result.
        Confirm,
        /// `⌘↵`: opens the selected result in a split.
        ConfirmInSplit,
        /// `⇥`: open kinds in all namespaces.
        ToggleAllNamespaces,
        /// Backspace in an empty prefixed query: back to searching everything.
        ClearMode,
        Dismiss,
    ]
);

/// The open palette, for toggling.
#[derive(Default)]
struct Current(Option<WeakEntity<CommandPalette>>);

impl Global for Current {}

/// Registers the palette's actions, key bindings and handlers.
pub fn init(cx: &mut App) {
    cx.default_global::<Current>();
    // The query input has focus; `> Input` makes these win over the input's own keys.
    let input = format!("{CONTEXT} > Input");
    let input = Some(input.as_str());
    let empty = format!("{EMPTY_CONTEXT} > Input");
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, input),
        KeyBinding::new("up", SelectPrevious, input),
        KeyBinding::new("ctrl-n", SelectNext, input),
        KeyBinding::new("ctrl-p", SelectPrevious, input),
        KeyBinding::new("enter", Confirm, input),
        KeyBinding::new("secondary-enter", ConfirmInSplit, input),
        KeyBinding::new("tab", ToggleAllNamespaces, input),
        KeyBinding::new("escape", Dismiss, input),
        KeyBinding::new("backspace", ClearMode, Some(&empty)),
    ]);

    cx.on_action(|_: &ToggleCommandPalette, cx| with_window(cx, toggle));
    cx.on_action(|action: &Open, cx| {
        let action = action.clone();
        with_window(cx, move |window, cx| {
            open(
                action.mode.unwrap_or_default(),
                action.query.as_deref().unwrap_or_default(),
                window,
                cx,
            )
        })
    });
    cx.on_action(|_: &GoToObject, cx| {
        with_window(cx, |window, cx| open(Mode::Objects, "", window, cx))
    });
    cx.on_action(|_: &GoToReference, cx| {
        if ResourceSelection::global(cx).is_empty() {
            NotificationCenter::push(cx, Notification::info("Select an object first."));
            return;
        }
        with_window(cx, |window, cx| open(Mode::References, "", window, cx))
    });
    cx.on_action(|_: &ShowAllNamespaces, cx| {
        let active = ActiveContext::global(cx).clone();
        ActiveContext::set(
            cx,
            ActiveContext {
                namespace: None,
                ..active
            },
        );
    });
    cx.on_action(|_: &AddNamespaceToFavorites, cx| {
        let active = ActiveContext::global(cx).clone();
        match (active.cluster, active.namespace) {
            (Some(cluster), Some(namespace)) => {
                kubyl_explorer::actions::add_favorite(&cluster.id, &namespace, cx)
            }
            _ => NotificationCenter::push(
                cx,
                Notification::info("Pick a namespace first: favorites are namespaces."),
            ),
        }
    });

    let list = Some("ResourceList");
    for spec in [
        ActionSpec::new("Palette: Resource Kinds…", Open::mode(Mode::Resources))
            .hint("Command")
            .bind(":", list),
        ActionSpec::new("Palette: Go to Object…", GoToObject).bind("secondary-p", None),
        ActionSpec::new("Palette: Actions…", Open::mode(Mode::Actions))
            .bind("secondary-shift-p", None),
        ActionSpec::new("Palette: Contexts…", Open::mode(Mode::Contexts)),
        ActionSpec::new("Palette: Namespaces…", Open::mode(Mode::Namespaces)),
        ActionSpec::new("Palette: Favorites…", Open::mode(Mode::Favorites)),
        ActionSpec::new("Resource: Go to Reference…", GoToReference).bind("shift-o", list),
        ActionSpec::new("Namespaces: Show All Namespaces", ShowAllNamespaces),
        ActionSpec::new("Favorites: Add Current Namespace", AddNamespaceToFavorites),
    ] {
        ActionRegistry::register(cx, spec);
    }
}

/// Runs `f` in the focused window. Deferred: global action handlers run while the dispatching
/// window is being updated, and updating it again from there fails.
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window
            && let Err(err) = window.update(cx, |_, window, cx| f(window, cx))
        {
            tracing::warn!("palette: no window: {err:#}");
        }
    });
}

fn current(cx: &App) -> Option<gpui::Entity<CommandPalette>> {
    cx.try_global::<Current>()?.0.as_ref()?.upgrade()
}

/// Opens the palette, or closes it when it is open.
pub fn toggle(window: &mut Window, cx: &mut App) {
    if current(cx).is_some() {
        window.close_dialog(cx);
        cx.set_global(Current(None));
    } else {
        open(Mode::All, "", window, cx);
    }
}

/// Opens the palette in `mode` with `query` (a leading prefix character picks the mode).
pub fn open(mode: Mode, query: &str, window: &mut Window, cx: &mut App) {
    if current(cx).is_some() {
        window.close_dialog(cx);
    }
    let origin = Origin {
        focus: window.focused(cx),
        contexts: window.context_stack(),
    };
    let view = cx.new(|cx| CommandPalette::new(mode, query, origin, window, cx));
    cx.set_global(Current(Some(view.downgrade())));
    let colors = cx.colors().clone();
    let dialog_view = view.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .w(gpui::px(600.))
            .margin_top(gpui::px(60.))
            .p_0()
            .bg(colors.panel)
            .close_button(false)
            .child(dialog_view.clone())
    });
    let focus = view.read(cx).query_focus(cx);
    window.focus(&focus, cx);
}

/// The keystrokes of `binding` in GPUI syntax (`secondary-k` is written as the platform key).
pub fn keystrokes_text(binding: &KeyBinding) -> String {
    binding
        .keystrokes()
        .iter()
        .map(|k| k.inner().unparse())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The keys that run `action` for an element with the key `contexts`, with GPUI's precedence:
/// the binding matching the deepest context wins (context-free bindings count as deepest), then
/// the one added last.
pub(crate) fn keys_for(
    keymap: &Keymap,
    action: &dyn Action,
    contexts: &[KeyContext],
) -> Option<String> {
    keymap
        .bindings_for_action(action)
        .enumerate()
        .filter_map(|(ix, b)| {
            let depth = match b.predicate() {
                Some(predicate) => predicate.depth_of(contexts)?,
                None => contexts.len(),
            };
            Some(((depth, ix), b))
        })
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, b)| keystrokes_text(b))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    gpui::actions!(test, [Logs]);

    #[gpui::test]
    fn keys_follow_the_context_and_precedence(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.bind_keys([
                KeyBinding::new("l", Logs, Some("ResourceList")),
                KeyBinding::new("secondary-l", Logs, Some("Workspace")),
            ]);
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            let list = KeyContext::parse("ResourceList").unwrap();
            let workspace = KeyContext::parse("Workspace").unwrap();
            assert_eq!(
                keys_for(&keymap, &Logs, std::slice::from_ref(&workspace)).as_deref(),
                Some(if cfg!(target_os = "macos") {
                    "cmd-l"
                } else {
                    "ctrl-l"
                })
            );
            assert_eq!(
                keys_for(&keymap, &Logs, &[workspace, list]).as_deref(),
                Some("l")
            );
            assert_eq!(keys_for(&keymap, &Logs, &[]), None);
        });
    }

    #[test]
    fn open_parses_from_keymap_json() {
        let open: Open = serde_json::from_value(serde_json::json!({"mode": "resources"})).unwrap();
        assert_eq!(open, Open::mode(Mode::Resources));
        assert!(serde_json::from_value::<Open>(serde_json::json!({"mode": "nope"})).is_err());
    }
}
