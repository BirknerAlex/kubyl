# Phase 04: YAML editor, schema validation, apply

**Status:** done (2026-09-24)
**Depends on:** 02 (and the 01 OpenAPI cache)
**Owns:** `crates/kubyl_yaml`
**Mockups:** board 3 · YAML editor with CRD schema

## Goal

View and edit **any** object (built-in or CRD) as YAML with schema-aware validation, hover docs and
completion, a diff against the live object, dry run, and server-side apply. Also create new objects
from templates or from pasted YAML.

## Tasks

### Editor
- [x] Code editor view based on `gpui-component`'s editor (not Zed's GPL `editor` crate): line numbers, tree-sitter YAML highlighting, folding, multi-cursor if available, search/replace, undo history
- [x] Gutter change markers against the live object (added, modified, removed)
- [x] Code lenses: "managedFields hidden · N managers" (toggle), "status · read-only · live" (the status section is dimmed and not editable)
- [x] Live-update handling: if the live object changes while there are local edits, show a banner ("Object changed on server — merge / reload / keep mine"). Clean buffers refresh silently
- [x] Secrets: base64 `data` is decoded for editing, with a toggle, and re-encoded on apply. Values are masked until revealed

### Schema intelligence
- [x] Build a schema per GVK from the OpenAPI v3 cache (built-ins and CRD `openAPIV3Schema`)
- [x] Validation with byte spans: unknown fields, wrong types, enum values, required fields, patterns, `x-kubernetes-*` hints. Inline diagnostics like Zed (squiggle plus an end-of-line message) and a Problems tab
- [x] Hover docs: field path, type, required/optional, description, source CRD/version
- [x] Completion: field names at the cursor path, enum values, `apiVersion`/`kind` pairs, references (names of Secrets, ConfigMaps, ServiceAccounts, StorageClasses, Issuers from the caches)
- [x] Schema outline dock: field tree with types and required markers. Clicking inserts or jumps
- [x] Related-objects list (from phase 02's relation resolver)

### Apply workflow
- [x] Diff vs live: unified and side-by-side, ignoring server-managed fields
- [x] Dry run (`dryRun=All`) shows the server's validation and admission-webhook errors inline
- [x] Apply with server-side apply (field manager `kubyl`), force-conflicts prompt that lists the conflicting managers. Fallback: replace or JSON merge patch for clusters/kinds that don't support SSA
- [x] PROD clusters: a confirm dialog showing the diff summary and cluster name
- [x] Revision history tab: for Deployments/StatefulSets/DaemonSets from ReplicaSets/ControllerRevisions; for everything else, a local history of Kubyl's own applies (stored locally, secrets excluded)

### Create
- [x] "New resource" from a kind picker: generates a skeleton from the schema's required fields plus common templates (Deployment, Service, Ingress, CronJob, ConfigMap, Secret, PVC). CRDs get a schema-derived skeleton
- [x] Multi-document YAML apply (paste or drop a file). Shows a per-document result list
- [x] Target the context and namespace explicitly in the header (avoids "applied to the wrong cluster")

## Acceptance criteria

- Editing the cert-manager `Certificate` from the mockup: `rotationPolicy: Allways` shows the enum
  error, hovering `renewBefore` shows its docs, dry run and apply work, and the diff shows exactly the edited lines.
- Works for a CRD installed after app start with no restart.
- Secrets never reach disk decoded (no temp files).

## Risks

- `gpui-component`'s editor may lack features we need (diagnostics rendering, inline widgets). Budget for extending it or upstreaming changes.
- YAML round-tripping must keep comments and key order in the user's buffer (edit text, not re-serialize).

## Handoff log

### 2026-09-24: phase 04 implemented

Screenshot: `design/screenshots/phase-04-yaml.png` (macOS, the Certificate from board 3 against
kind: three edits, the enum error, hover on `renewBefore`, diff, schema outline, related objects).

**What exists** (`crates/kubyl_yaml`, wired in through `init`):
- `view::YamlEditor` (`ViewKind::Yaml`): object ref = edit, list ref (kind + namespace) or no
  target = new resource. Rendering in `ui.rs`: toolbar (crumb, explicit cluster · namespace chip,
  PROD/read-only badges, "N changes vs live", Revert, Diff vs live, Dry run, Apply), banner, the
  editor, bottom panel (Diff vs live unified/side by side, Problems, Revision history, Dry
  run/Apply results) and a 300 px sidebar (schema outline, related objects, editor toggles).
- Editor: gpui-component `EditorState` with the `tree-sitter-yaml` feature (highlighting, folding,
  line numbers, search/replace `secondary-f`/`secondary-shift-f`, undo, multi-cursor
  `secondary-alt-up/down`). Diagnostics use its `DiagnosticSet` (squiggles); gutter markers,
  code lenses and end-of-line messages are painted by a `canvas` over the editor from
  `EditorState::range_to_bounds`. The status block is dimmed with a decoration collection.
- Model: `parse` (spanned YAML on `granit-parser`, paths, YAML 1.2 core JSON), `schema` (merged
  `$ref`/`allOf` view of OpenAPI v3 nodes, `Schemas` cache per cluster + group-version, dropped on
  `DiscoveryChanged`), `validate`, `intel` (hover + completion), `diff` (comment-insensitive
  line diff that ignores server-managed fields, markers, hunks, side by side, three-way merge
  via `similar`), `render` (object → text with keys sorted like kubectl, managedFields toggle,
  Secret masking), `apply` (prepare documents, SSA/dry run/force, 415 fallback, server errors
  placed on their fields), `templates`, `settings` (`yaml_editor` section, `yaml_history` state).
- Actions (ActionRegistry, so the palette `>` and keymaps see them): `Resource: Edit YAML` (`e`,
  `ResourceList`, available for objects), `Resource: New…` (`secondary-n`, global, hidden on
  read-only clusters), and in `YamlEditor`: Apply `secondary-s`, Dry Run `secondary-shift-s`,
  Diff `secondary-shift-d`, Problems `secondary-shift-m`, Next Problem `f8`, History
  `secondary-shift-h`, panel `secondary-j`, Revert `secondary-alt-z`, Reveal/Mask secrets
  `secondary-shift-r`, managedFields `secondary-alt-m`… (modifier keys only: the buffer takes
  text).
- Shared-crate changes (own commits): `kubyl_resources::ops::{workload_history, workload_undo,
  WITH_HISTORY}` (StatefulSet/DaemonSet ControllerRevisions; the explorer's `u` now rolls those
  back too, phase 02 open item closed), `kubyl_explorer::dialogs` and
  `kubyl_palette::references` public, `kubyl_ui` editor colors, screenshot steps `mouse=`/`click=`.

**Verified against kind** (`script/dev-cluster.sh`; sandbox HOME/config; typed and clicked
through the screenshot harness):
- Certificate from the mockup (applied first with field manager `helm`): `rotationPolicy:
  Allways` shows `Unsupported value “Allways”: supported values are “Never”, “Always”` as a
  squiggle, end-of-line message and in Problems; hovering `renewBefore` shows path, type,
  optional, the CRD description and "From CRD certificates.cert-manager.io · openAPIV3Schema v1".
- Dry run: SSA conflicts with `helm` open the force dialog listing the fields; forcing the dry run
  returns the server's enum error, placed on `Allways` (source "server").
- Apply (valid edit: `renewBefore: 720h # 30d`, second dnsName): force → applied, `kubectl` shows
  both changes and `kubyl` as a manager; the buffer keeps the comment and shows "no changes".
  The diff showed exactly the edited lines (+1 added, 1 modified) under `@@ spec @@`.
- CRD installed while the app ran (`widgets.late.kubyl.dev`, tab restored before it existed):
  the object loaded when it appeared and the CRD's schema validated it (enum, types), no restart.
- Live change while editing (`kubectl patch` of `commonName`): banner, Merge combined it with the
  local `renewBefore` edit.
- PROD (`kubernetes.contexts.<id>.production`): Apply opens the confirmation with the change
  summary (`~ spec.renewBefore`) and stays disabled until `api-tls` is typed (not submitted).
- Secret `checkout-db`: values masked, `secondary-shift-r` shows `not-a-real-password`; kubectl's
  last-applied annotation (which holds the whole Secret) is masked too. `grep` over the config
  dir, HOME and the log found neither the decoded nor the base64 value.
- New resource: `secondary-n` from the Certificates list opens a Certificate skeleton with the
  CRD's required fields (`spec.issuerRef.name`, `spec.secretName`); from Welcome the picker lists
  templates and every creatable kind (`conf` → ConfigMap). Two-document apply: per-document
  results (one applied, one rejected with the server message and a schema hint).
- StatefulSet/DaemonSet history and undo: `crates/kubyl_resources/tests/live.rs`
  (`statefulset_history_and_undo`, ignored by default).
- Unit tests: parser spans/paths/JSON, schema refs and labels, validation messages, diff
  (exact lines, comments, server fields, merge), secrets round trip, managedFields toggle,
  templates/skeletons, completion/hover, conflicts, server error placement, history caps.

**Not verified / stubbed / deviations.**
- Not clicked by hand: the managedFields lens (the pill and `secondary-alt-m` toggle it too),
  schema outline insert/jump, related-object links, side-by-side diff, "Load into editor" of the
  local history, "Roll back" in the history tab, Reload/Keep mine (Merge was), completion popup
  selection (the popup opened while typing; items are unit-tested), multi-cursor, replace.
- The 415 replace/create fallback isn't exercised (kind supports SSA).
- `status` is dimmed and never sent (stripped before apply), but it can be typed into: the editor
  has no read-only ranges.
- Code lenses sit at the end of the `metadata:`/`status:` lines instead of on their own line
  (the editor has no virtual lines); end-of-line messages and gutter bars are painted overlays.
- The hover popup uses the editor's mono font (gpui-component renders hover markdown there) and
  shows the description's first paragraph. The Apply button says "Apply" (not "(server-side)")
  so the toolbar fits next to the sidebar at 1440 px.
- The schema outline is a column of the tab, not a right-dock panel. Edits to server-managed
  fields (`resourceVersion`, `generation`…) and comment-only edits don't count as changes.
- Undo in the explorer's rollout picker now also lists StatefulSet/DaemonSet ControllerRevisions.

**Fix to an earlier phase found while testing** (own commit): the explorer's dialogs passed
their view as a child, which gpui-component puts in a collapsing scroll body; presses on the
footer reached the backdrop and closed the dialog, so Delete/Scale/Drain/Roll back buttons never
confirmed (phase 02/03 had not clicked them). Now `.content(..)`.

**Gotchas.**
- The screenshot harness splits steps at commas; `keys=` typing into the editor must close the
  completion menu (`escape`) before `enter`, and `shift-3`/`shift-;` produce `3`/`;` (no key
  char): type `#` and `:` directly.
- GPUI click handlers need a frame between mouse down and up (the harness draws one).
- The API server serializes `metadata` fields in its own order; `render::sorted` sorts keys so
  diffs are stable (serde_json has `preserve_order` enabled in the tree).
- Windows/Linux: only compiled and unit-tested in CI (tree-sitter builds there); nothing
  platform-specific in the crate. `secondary-*` is Ctrl there. Editor keys avoid gpui-component's input
  bindings (`secondary-shift-f` is Replace, `secondary-shift-z` Redo, `ctrl-f`/`ctrl-h` search).

**API for later phases (08 Operators: install YAML and diffs).**
- Open an editor: `OpenView(ViewRequest::for_resource(ViewKind::Yaml, object_ref))`, or a new
  resource with `ResourceRef::list(cluster, gvr, Some(ns))` / `ViewRequest::new(ViewKind::Yaml)`.
- Without the view: `parse::parse(text)` → `Parsed { docs, error }`; `apply::prepare(&parsed,
  &discovery, default_ns, None)` → `Vec<Prepared>`; `apply::apply_all(client, docs,
  Options { dry_run, force })` (run with `spawn_kube`) → per-document `Outcome`
  (`Applied`/`Conflicts`/`Failed { causes }`); `apply::server_problems` maps failures to ranges.
- Diffs: `diff::diff(old, new, context)` → hunks, gutter markers, `summary` (`~ spec.x`);
  `diff::side_by_side(&hunk)`; `diff::merge(base, ours, theirs)`.
- Rendering an object like the editor does: `render::render(&object, RenderOptions::default())`
  (Secrets come back masked with their `SecretValues`; `render::restore_secret` before sending).
- Schemas: `schema::Schemas::global(cx)` (`get(cluster, group, version, cx)`, observe for loads),
  `Schema::for_gvk(&doc, &gvk)`, `validate::validate(root, &schema)`.
- Confirmations: `kubyl_explorer::dialogs::confirm(ConfirmSpec { typed, lines, note, .. })`.
- Rollout history: `kubyl_resources::ops::workload_history/workload_undo`.

#### 2026-09-25: Details pane grew an inline YAML sub-tab (owner request, out of phase order)

At the repo owner's explicit request, `kubyl_explorer`'s Details pane
(`crates/kubyl_explorer/src/details.rs`) now offers a YAML sub-tab next to Summary/Describe,
building this crate's `ViewKind::Yaml` view inline via `ViewRegistry::build` rather than
`OpenView`. `kubyl_yaml` itself (this file's crate) is unchanged. See
plans/02-resource-explorer.md's 2026-09-25 entry for the full description, including the new
Secret value reveal/copy UI added alongside it (unrelated to YAML, same commit range).
