//! Keymaps: the Zed-like default and the k9s-compatible preset (`assets/keymaps/`), plus the
//! user's bindings from settings.json:
//!
//! ```json
//! "keymap": {
//!   "preset": "k9s",
//!   "bindings": [
//!     { "context": "ResourceList", "bindings": { "x": "resource::Delete", "ctrl-d": null } }
//!   ]
//! }
//! ```
//!
//! Crates register their own default bindings in code (`ActionRegistry::register`,
//! `cx.bind_keys`). This crate layers the preset files and then the user's bindings on top, so
//! later layers win; `null` unbinds a key. Changes to settings.json apply immediately. After
//! every change the keystrokes in the `ActionRegistry` (key-hint bar, palette) are updated to
//! what is really bound.
//!
//! Precedence: GPUI prefers the binding that matches the deepest key context, then the one added
//! last. Presets are tagged with meta 1 and user bindings with meta 0 so that a user's `null`
//! also hides preset and built-in bindings, while reloading can tell them apart from the
//! built-in ones.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    Action, App, KeyBinding, KeyBindingContextPredicate, KeyBindingMetaIndex, Keymap, NoAction,
    SharedString,
};
use kubyl_core::{ActionRegistry, Notification, NotificationCenter};
use kubyl_settings::{Settings, SettingsSection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_KEYMAP: &str = include_str!("../../../assets/keymaps/default.json");
const K9S_KEYMAP: &str = include_str!("../../../assets/keymaps/k9s.json");

/// Meta of user bindings (strongest: their `null` hides everything else).
const USER: KeyBindingMetaIndex = KeyBindingMetaIndex(0);
/// Meta of preset bindings.
pub const PRESET: KeyBindingMetaIndex = KeyBindingMetaIndex(1);

/// Which keymap file is layered over the built-in bindings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Zed-like: `⌘K` palette, `⌘P` go to object, `⌘[`/`⌘]` back and forward.
    #[default]
    Default,
    /// The default plus k9s keys: `?` actions, `0` all namespaces, `[`/`]`/`-`/`esc` history,
    /// `ctrl-u` page up.
    K9s,
}

/// Bindings in one key context, like a block of Zed's keymap.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct KeymapBlock {
    /// Key context predicate, e.g. `ResourceList`, `ResourceList && kind == Pod`, `Pane`.
    /// Empty: everywhere.
    pub context: Option<String>,
    /// Keystrokes (`ctrl-d`, `g g`, `secondary-k`) to an action name (`resource::Delete`), an
    /// action with arguments (`["palette::Open", {"mode": "resources"}]`) or `null` to unbind.
    pub bindings: BTreeMap<String, Value>,
}

/// The `"keymap"` section of settings.json.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct KeymapSettings {
    pub preset: Preset,
    /// Your bindings, applied after the preset.
    pub bindings: Vec<KeymapBlock>,
}

impl SettingsSection for KeymapSettings {
    const KEY: Option<&'static str> = Some("keymap");
}

/// Registers the settings section and applies the keymap once every crate has registered its
/// bindings (on the first turn of the event loop), then again whenever settings.json changes.
pub fn init(cx: &mut App) {
    Settings::register::<KeymapSettings>(cx);
    cx.spawn(async move |cx| cx.update(apply)).detach();
    Settings::observe::<KeymapSettings>(cx, |_, cx| apply(cx)).detach();
}

/// Rebuilds the keymap: built-in bindings, then the preset, then the user's bindings.
pub fn apply(cx: &mut App) {
    let settings = Settings::get::<KeymapSettings>(cx).clone();
    let builtin: Vec<KeyBinding> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|b| b.meta().is_none())
        .cloned()
        .collect();
    let mut errors = Vec::new();
    let mut layered = Vec::new();
    for source in preset_sources(settings.preset) {
        match parse_file(source) {
            Ok(blocks) => layered.extend(load_blocks(&blocks, PRESET, cx, &mut errors)),
            Err(err) => errors.push(format!("preset: {err}")),
        }
    }
    layered.extend(load_blocks(&settings.bindings, USER, cx, &mut errors));

    cx.clear_key_bindings();
    cx.bind_keys(builtin);
    cx.bind_keys(layered);
    sync_registry(cx);

    for error in &errors {
        tracing::warn!("keymap: {error}");
    }
    if !errors.is_empty() {
        NotificationCenter::push(
            cx,
            Notification::warning(format!("Keymap: {}", errors.join("; "))),
        );
    }
}

/// The keymap files a preset consists of, in layering order.
pub fn preset_sources(preset: Preset) -> Vec<&'static str> {
    match preset {
        Preset::Default => vec![DEFAULT_KEYMAP],
        Preset::K9s => vec![DEFAULT_KEYMAP, K9S_KEYMAP],
    }
}

/// Parses a keymap file: a JSON array of blocks; lines starting with `//` are comments.
pub fn parse_file(source: &str) -> Result<Vec<KeymapBlock>, String> {
    let json: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&json).map_err(|err| err.to_string())
}

/// Turns blocks into key bindings. Problems (unknown actions, bad keystrokes or contexts) are
/// collected in `errors`; the rest still loads.
pub fn load_blocks(
    blocks: &[KeymapBlock],
    meta: KeyBindingMetaIndex,
    cx: &App,
    errors: &mut Vec<String>,
) -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for block in blocks {
        let context = block
            .context
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty());
        let predicate = match context.map(KeyBindingContextPredicate::parse) {
            None => None,
            Some(Ok(predicate)) => Some(Rc::new(predicate)),
            Some(Err(err)) => {
                errors.push(format!("context {:?}: {err}", context.unwrap_or_default()));
                continue;
            }
        };
        for (keys, value) in &block.bindings {
            let action = match build_action(value, cx) {
                Ok(action) => action,
                Err(err) => {
                    errors.push(format!("{keys}: {err}"));
                    continue;
                }
            };
            let input = (!value.is_string() && !value.is_null())
                .then(|| SharedString::from(value.to_string()));
            match KeyBinding::load(
                keys,
                action,
                predicate.clone(),
                false,
                input,
                &gpui::DummyKeyboardMapper,
            ) {
                Ok(binding) => bindings.push(binding.with_meta(meta)),
                Err(err) => errors.push(format!("{keys}: {err}")),
            }
        }
    }
    bindings
}

/// `"resource::Delete"`, `["palette::Open", {"mode": "resources"}]` or `null` (unbind).
fn build_action(value: &Value, cx: &App) -> Result<Box<dyn Action>, String> {
    let (name, data) = match value {
        Value::Null => return Ok(Box::new(NoAction)),
        Value::String(name) => (name.as_str(), None),
        Value::Array(items) => match items.as_slice() {
            [Value::String(name)] => (name.as_str(), None),
            [Value::String(name), data] => (name.as_str(), Some(data.clone())),
            _ => return Err("expected [\"action::Name\", { arguments }]".into()),
        },
        _ => return Err("expected an action name, [name, arguments] or null".into()),
    };
    cx.build_action(name, data)
        .map_err(|err| format!("{name}: {err}"))
}

/// Updates the keystrokes the `ActionRegistry` shows (key-hint bar, palette) to the bindings
/// that are really active: for an action registered with a context, the last binding in that
/// exact context, else the last binding at all. Unbound actions lose their keystrokes.
pub fn sync_registry(cx: &mut App) {
    let keymap = cx.key_bindings();
    let updates: Vec<(usize, Option<SharedString>)> = {
        let keymap = keymap.borrow();
        ActionRegistry::global(cx)
            .all()
            .iter()
            .enumerate()
            .map(|(ix, spec)| {
                let context = spec
                    .context
                    .as_deref()
                    .and_then(|c| KeyBindingContextPredicate::parse(c).ok());
                (
                    ix,
                    displayed_keys(&keymap, &*spec.action(), context.as_ref()),
                )
            })
            .collect()
    };
    for (ix, keys) in updates {
        if ActionRegistry::global(cx).all()[ix].keystrokes != keys {
            ActionRegistry::set_keystrokes(cx, ix, keys);
        }
    }
}

fn displayed_keys(
    keymap: &Keymap,
    action: &dyn Action,
    context: Option<&KeyBindingContextPredicate>,
) -> Option<SharedString> {
    let bindings: Vec<&KeyBinding> = keymap.bindings_for_action(action).collect();
    let in_context = bindings
        .iter()
        .rev()
        .find(|b| b.predicate().as_deref() == context);
    let binding = in_context.or_else(|| bindings.last())?;
    Some(
        binding
            .keystrokes()
            .iter()
            .map(|k| k.inner().unparse())
            .collect::<Vec<_>>()
            .join(" ")
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{KeyContext, TestAppContext};
    use kubyl_core::ActionSpec;

    use super::*;

    gpui::actions!(test_keymap, [Logs, Delete, Help]);

    fn setup(cx: &mut TestAppContext) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            kubyl_core::init(cx);
            kubyl_settings::init_with_dir(cx, dir.path());
            ActionRegistry::register(
                cx,
                ActionSpec::new("Logs", Logs)
                    .hint("Logs")
                    .bind("l", Some("ResourceList")),
            );
            ActionRegistry::register(
                cx,
                ActionSpec::new("Delete", Delete)
                    .hint("Delete")
                    .bind("ctrl-d", Some("ResourceList")),
            );
            ActionRegistry::register(cx, ActionSpec::new("Help", Help));
            init(cx);
        });
        cx.run_until_parked();
        dir
    }

    fn action_for(cx: &App, keys: &str, context: &str) -> Option<&'static str> {
        let keystrokes: Vec<gpui::Keystroke> = keys
            .split_whitespace()
            .map(|k| gpui::Keystroke::parse(k).unwrap())
            .collect();
        let contexts = vec![KeyContext::parse(context).unwrap()];
        let (bindings, _) = cx
            .key_bindings()
            .borrow()
            .bindings_for_input(&keystrokes, &contexts);
        bindings.first().map(|b| b.action().name())
    }

    #[test]
    fn preset_files_parse() {
        for source in [DEFAULT_KEYMAP, K9S_KEYMAP] {
            let blocks = parse_file(source).unwrap();
            assert!(!blocks.is_empty());
        }
    }

    #[gpui::test]
    fn user_bindings_override_and_unbind(cx: &mut TestAppContext) {
        let _dir = setup(cx);
        cx.update(|cx| {
            assert_eq!(
                action_for(cx, "l", "ResourceList"),
                Some("test_keymap::Logs")
            );
            Settings::update::<KeymapSettings>(cx, |s| {
                s.bindings = vec![KeymapBlock {
                    context: Some("ResourceList".into()),
                    bindings: BTreeMap::from([
                        ("shift-l".into(), Value::String("test_keymap::Logs".into())),
                        ("ctrl-d".into(), Value::Null),
                        ("?".into(), Value::String("test_keymap::Help".into())),
                        ("x".into(), Value::String("nope::Missing".into())),
                    ]),
                }];
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            // Both keys run Logs; the hint shows the user's binding.
            assert_eq!(
                action_for(cx, "l", "ResourceList"),
                Some("test_keymap::Logs")
            );
            assert_eq!(
                action_for(cx, "shift-l", "ResourceList"),
                Some("test_keymap::Logs")
            );
            assert_eq!(action_for(cx, "ctrl-d", "ResourceList"), None);
            let hints = ActionRegistry::global(cx).hints("ResourceList");
            assert_eq!(hints, vec![("shift-l".into(), "Logs".into())]);
            let help = &ActionRegistry::global(cx).all()[2];
            assert_eq!(help.keystrokes.as_deref(), Some("?"));
            // The unknown action is reported, the rest loaded.
            assert!(
                NotificationCenter::global(cx)
                    .since(0)
                    .any(|(_, n)| n.message.contains("nope::Missing"))
            );

            // Removing the override restores the built-in bindings.
            Settings::update::<KeymapSettings>(cx, |s| s.bindings.clear());
        });
        cx.run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                action_for(cx, "ctrl-d", "ResourceList"),
                Some("test_keymap::Delete")
            );
            assert_eq!(action_for(cx, "shift-l", "ResourceList"), None);
            let hints = ActionRegistry::global(cx).hints("ResourceList");
            assert_eq!(hints.len(), 2);
        });
    }

    #[gpui::test]
    fn presets_switch(cx: &mut TestAppContext) {
        let _dir = setup(cx);
        cx.update(|cx| {
            // Actions of other crates aren't registered here: preset entries naming them are
            // reported and skipped, the rest of the preset loads.
            let mut errors = Vec::new();
            let blocks = parse_file(K9S_KEYMAP).unwrap();
            let loaded = load_blocks(&blocks, PRESET, cx, &mut errors);
            assert!(!errors.is_empty());
            assert!(loaded.iter().all(|b| b.meta() == Some(PRESET)));
            Settings::update::<KeymapSettings>(cx, |s| s.preset = Preset::K9s);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            // Built-in bindings survive the preset switch.
            assert_eq!(
                action_for(cx, "l", "ResourceList"),
                Some("test_keymap::Logs")
            );
        });
    }
}
