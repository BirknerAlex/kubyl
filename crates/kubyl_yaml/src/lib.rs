//! YAML editor, schema validation, diff and apply (board 3 · YAML).
//!
//! - [`view::YamlEditor`]: `ViewKind::Yaml` for an object (edit) or a list/nothing (new
//!   resource); rendering in `ui`.
//! - [`parse`]: YAML with byte spans (`granit-parser`), paths, JSON conversion.
//! - [`schema`]: OpenAPI v3 schemas per kind (built-ins and CRDs) and their shared cache.
//! - [`validate`]: schema validation with ranges for inline diagnostics.
//! - [`intel`]: hover docs and completion (adapted to the editor in `lsp`).
//! - [`diff`]: diffs against the live object, gutter markers, three-way merge.
//! - [`render`]: object → editor text (managedFields, Secret masking) and back.
//! - [`apply`]: dry run and server-side apply of the buffer's documents.
//! - [`templates`]: "New resource" templates and schema skeletons.
//! - [`settings`]: the `yaml_editor` settings and the local apply history.
//!
//! Actions: `yaml::EditYaml` (`e` in lists), `yaml::NewResource` (`secondary-n`), and inside the
//! editor (`YamlEditor` context) `yaml::Apply` (`secondary-s`), `yaml::DryRun`
//! (`secondary-shift-s`), `yaml::ShowDiff`, `yaml::ShowProblems`, `yaml::NextProblem` (`f8`)…

pub mod apply;
pub mod diff;
pub mod intel;
mod lsp;
pub mod parse;
pub mod render;
pub mod schema;
pub mod settings;
pub mod templates;
mod ui;
pub mod validate;
pub mod view;

use gpui::{App, AppContext as _, Window, actions};
use kubyl_core::actions::OpenView;
use kubyl_core::{ActionRegistry, ActionSpec, ResourceRef, ViewKind, ViewRegistry, ViewRequest};
use kubyl_resources::ResourceSelection;
use kubyl_settings::Settings;

use view::{
    Apply, CONTEXT, DryRun, NextProblem, Revert, ShowDiff, ShowHistory, ShowProblems,
    ShowTemplates, ToggleManagedFields, TogglePanel, ToggleSecrets, ToggleSideBySide, YamlEditor,
};

actions!(
    yaml,
    [
        /// Opens the selected object in the YAML editor.
        EditYaml,
        /// Opens a new-resource editor (for the list's kind when one is focused).
        NewResource,
    ]
);

/// Registers the editor view, its actions and settings.
pub fn init(cx: &mut App) {
    Settings::register::<settings::YamlSettings>(cx);
    schema::Schemas::install(cx);

    ViewRegistry::register(cx, ViewKind::Yaml, |request, window, cx| {
        let target = request.target.clone();
        Some(Box::new(cx.new(|cx| YamlEditor::new(target, window, cx))))
    });

    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: Edit YAML", EditYaml)
            .hint("Edit")
            .bind("e", Some("ResourceList"))
            .available_when(|target, _| target.is_object()),
    );
    ActionRegistry::register(
        cx,
        ActionSpec::new("Resource: New…", NewResource)
            .bind("secondary-n", None)
            .available_when(|_, caps| !caps.read_only),
    );
    let editor = Some(CONTEXT);
    for spec in [
        ActionSpec::new("YAML: Apply (Server-Side)", Apply).bind("secondary-s", editor),
        ActionSpec::new("YAML: Dry Run", DryRun).bind("secondary-shift-s", editor),
        ActionSpec::new("YAML: Diff vs Live", ShowDiff).bind("secondary-shift-d", editor),
        ActionSpec::new("YAML: Show Problems", ShowProblems).bind("secondary-shift-m", editor),
        ActionSpec::new("YAML: Next Problem", NextProblem).bind("f8", editor),
        ActionSpec::new("YAML: Revision History", ShowHistory).bind("secondary-shift-h", editor),
        ActionSpec::new("YAML: Toggle Bottom Panel", TogglePanel).bind("secondary-j", editor),
        ActionSpec::new("YAML: Revert to Live", Revert).bind("secondary-alt-z", editor),
        ActionSpec::new("YAML: Reveal/Mask Secret Values", ToggleSecrets)
            .bind("secondary-shift-r", editor),
        ActionSpec::new("YAML: Show/Hide managedFields", ToggleManagedFields)
            .bind("secondary-alt-m", editor),
        ActionSpec::new("YAML: Toggle Side-by-Side Diff", ToggleSideBySide)
            .bind("secondary-alt-d", editor),
        ActionSpec::new("YAML: New Resource Templates", ShowTemplates)
            .bind("secondary-shift-t", editor),
    ] {
        ActionRegistry::register(cx, spec);
    }

    cx.on_action(|_: &EditYaml, cx| {
        let Some(target) = ResourceSelection::global(cx)
            .primary()
            .map(|s| s.target.clone())
            .filter(ResourceRef::is_object)
        else {
            return;
        };
        open(ViewRequest::for_resource(ViewKind::Yaml, target), cx);
    });
    cx.on_action(|_: &NewResource, cx| {
        // From a list: its kind and namespace; otherwise the active cluster and namespace.
        let request = match ResourceSelection::global(cx).primary() {
            Some(selected) => ViewRequest::for_resource(
                ViewKind::Yaml,
                ResourceRef::list(
                    selected.target.cluster.clone(),
                    selected.target.gvr.clone(),
                    selected.target.namespace.clone(),
                ),
            ),
            None => ViewRequest::new(ViewKind::Yaml),
        };
        open(request, cx);
    });
}

/// Opens a view in the focused window. Deferred: global action handlers run while the
/// dispatching window is being updated.
fn open(request: ViewRequest, cx: &mut App) {
    cx.defer(move |cx| {
        let window = cx.active_window().or_else(|| cx.windows().first().copied());
        if let Some(window) = window {
            window
                .update(cx, |_, window: &mut Window, cx| {
                    window.dispatch_action(Box::new(OpenView(request)), cx)
                })
                .ok();
        }
    });
}
