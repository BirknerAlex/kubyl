# Phase 03: Command palette, navigation, keymaps

**Status:** done (2026-09-24; scroll-position restore left open, see Handoff log)
**Depends on:** 02
**Owns:** `crates/kubyl_palette`, `crates/kubyl_keymap`
**Mockups:** board 6 · Command palette

## Goal

Everything is reachable from the keyboard in two keystrokes, like k9s's `:` command mode and
Zed's command palette.

## Tasks

### Palette
- [x] `⌘K` / `Ctrl+K` opens it. `:` in a list view opens it pre-filled in resource mode
- [x] Modes by prefix: `:` resource kinds (names, short names, plurals, categories such as `all`, CRDs included), `@` contexts, `#` namespaces, `>` actions (from `ActionRegistry`, scoped to the current selection), `*` favorites, `/` filter the current view. Mode chips are clickable
- [x] Fuzzy matching (`nucleo` crate) with match highlighting, grouped results, recency boost
- [x] Result metadata: API group/version, aliases, live count, scope
- [x] `↵` open, `⌘↵` open in split, `⇥` toggle all namespaces, `esc` close. The footer shows these hints
- [x] k9s-style inline commands: `:pods payments`, `:deploy -A`, `:ctx staging-eu-west-1`, `:ns kube-system`, `:xray deploy` (later phase), `:q`

### Navigation
- [x] Back/forward history per pane (`⌘[`, `⌘]`)
- [x] Jump from a reference to its object (owner, selector target, secret in a volume) anywhere
- [x] "Go to object": `⌘P`-like quick open by name across the loaded caches of all connected clusters
- [ ] Restore tabs, splits and scroll positions per window across restarts (`state.json`): tabs and splits were restored since phase 00; split sizes, scroll positions and per-window layouts are still open

### Keymaps
- [x] Keymap files in `assets/keymaps/`: `default.json` (Zed-like) and `k9s.json` (k9s-compatible single keys and `ctrl-*`). User overrides in `keymap.json`
- [x] Context-aware bindings (list view vs. editor vs. terminal), shown in menus, tooltips and the key-hints bar
- [x] Vim-style list navigation (`gg`, `G`, `ctrl-d/u`) in the k9s preset

## Acceptance criteria

- Typing `:cert` shows Certificates first (from a CRD) with the count. `↵` opens the list.
- Every action in the key-hints bar is also findable in `>` mode.
- A k9s user can switch context, namespace and kind without touching the mouse.

## Handoff log

### 2026-09-24: phase 03 implemented

Screenshot: `design/screenshots/phase-03-palette.png` (macOS, `:cert` against kind).

**What exists.**
- `kubyl_palette`: the palette (a gpui-component dialog, 600 px, board 6). Modes by prefix
  (`: @ # > * /`, chips clickable, backspace in an empty query leaves the mode), plus two
  modes without a prefix: Objects ("Go to object", `secondary-p`) and References
  (`shift-o` in lists). No prefix searches kinds, actions, contexts, namespaces, favorites and
  (from 2 characters) loaded objects, grouped; empty shows Recent.
  - `:` kinds of the active cluster from discovery (plural, singular, kind, short names,
    `plural.group` typed in full, categories: `:all`), CRDs included. Metadata: api version,
    `cluster` for cluster-scoped kinds, aliases, live count (first 8 results: borrows a list's
    store, else a metadata watch held while the palette is open, 150 ms debounce). Next to
    kinds: actions and favorites that contain the query. Inline commands: `:pods payments`,
    `:po -n ns`, `:deploy -A`, `:ctx [name]`, `:ns [name]`, `:q`, `:xray` (toast: later phase).
    `-A`/`⇥` and a named namespace also set the active namespace, like k9s.
  - `>` every `ActionRegistry` action whose key context matches where the palette was opened
    (the focused element's context stack is captured on open) and that is available for
    `ResourceSelection::primary()` (caps + cached RBAC, like the hint bar). Actions of the
    focused list come first. Keys shown are the real bindings for that context (GPUI
    precedence: deepest context, then last added). Running an action restores focus first and
    dispatches it there, so element handlers and global handlers both work.
  - `*` favorites (open with `kubyl_explorer::sidebar::open_favorite`), "Add <ns> to favorites"
    (`kubyl_explorer::actions::add_favorite`, hidden when it already is one) and "Favorites
    workspace: <kind>" for any namespaced kind (`ViewKind::Custom("favorites")`).
  - `/` sets the focused list's filter (`kubyl_explorer::list::SetFilter`), empty clears it.
  - References: owner refs, a pod's node, service account, pull secrets, volumes (secret,
    config map, PVC, projected), env/envFrom refs, a selector's pods (list filtered with the
    labels), a node's pods (`spec.nodeName=`), PVC→PV/StorageClass, PV→claim, Ingress→services,
    TLS secrets, class, bindings→role/subjects, HPA→target, Event→object. Unknown kinds show a
    notice. The details dock's clickable owner/secret links (phase 02) remain.
  - Matching: `nucleo-matcher` 0.3 (MPL-2.0, allowed by deny.toml), exact alias +1000, prefix
    +200, matches under 18 points per query character dropped, match highlights, recency boost
    from `state.json` → `palette.keys` (50 keys, no secrets).
  - Keys inside the palette (`CommandPalette > Input`): ↑/↓, ctrl-n/p, ↵, ⌘↵ (opens in a split),
    ⇥ (all namespaces), esc.
- `kubyl_keymap`: `assets/keymaps/default.json` (Zed-like app keys) and `k9s.json` (on top of
  the default: `?` actions, `0` all namespaces, `[`/`]`/`-`/`esc` history, `ctrl-u` page up,
  `:`/`?` in the tree). Settings: `"keymap": { "preset": "default" | "k9s", "bindings": [ {
  "context", "bindings": { "keys": "action::Name" | ["action::Name", {args}] | null } } ] }`
  in settings.json (hot-reloaded; problems are toasted and the rest loads). Applied on the
  first event-loop turn after every crate's init. After each apply the `ActionRegistry`
  keystrokes follow the real bindings (`ActionRegistry::set_keystrokes`), so the hint bar and
  the palette show overrides and hide unbound keys.
- Shell (`kubyl`): per-pane back/forward (`pane::GoBack`/`GoForward`, `secondary-[`/`]`,
  `alt-left`/`right`, Go menu), reopening closed tabs from their `ViewRequest`. App, workspace
  and pane actions are registered in the `ActionRegistry` (so `>` lists them).
- Explorer: `List: Filter/Mark Row/Select All/Toggle Wide Columns` in the registry, `:` and `/`
  stay last in the hint bar, `SetFilter` action, `list::open_filtered`.

**Verified against kind** (`script/dev-cluster.sh`, sandbox HOME/config, keys typed through
the screenshot harness, see below):
- `:cert` → certificates (CRD) first with count 1, then the other certificate kinds; `↵` opens.
- `:po payments ↵` opens Pods in payments and switches the title bar namespace.
- `#kube-sys ↵` switches namespace, lists follow. `@` lists contexts with state/latency.
- `/check ↵` filters the Deployments list. `⌘[` goes back to the previous tab.
- `:svc ⌘↵` splits: old tab left, Services in the new right pane.
- `⌘P checkout` lists the deployment, replica sets and pods by name across loaded caches.
- `shift-o` on a deployment offers "Pods app=checkout-api".
- `>delete ↵` opens the Delete dialog for the selection.
- k9s preset with a user override (`x` → Delete, `ctrl-d` → null): `esc` goes back, `?` opens
  actions, the hint bar shows `x Delete`.
- `*` → "Add payments to favorites" adds it (toast, sidebar row), then lists it and the
  Favorites workspace.
- Unit tests: matcher, inline commands, ranking/grouping/recency for every mode, references,
  keymap layering/unbind/hot reload/registry sync, pane history, presets name real actions.

**Not done / stubbed.**
- Restoring scroll positions and split sizes, and per-window layouts (all windows share the
  `workspace` key) are open. Needs a `TabView` hook for view state; tabs/splits restore fine.
- Tooltips don't show keys (the title-bar search pill shows a fixed `⌘K`). Menus show the
  bound keys (GPUI does that from the keymap), the hint bar follows the keymap.
- `ctrl-d` stays Delete in the k9s preset (as in k9s), so vim's half-page `ctrl-d` isn't bound;
  `ctrl-u` pages up, `ctrl-f`/`ctrl-b`, `g g`, `G`, `j`/`k` were already bound by the explorer.
- Live counts show for the first 8 kinds only; a Favorites result has no count.
- `:xray` is a placeholder toast.
- Bindings added by a crate *after* the first event-loop turn are kept on reload but land
  after the user's layer (none exist today).

**Fixes to earlier phases found while testing** (separate commits):
- Explorer dialogs (delete, scale, undo, drain), the namespace switcher, the Favorites
  workspace action and the sidebar filter did nothing when triggered by key or action: global
  action handlers run while the window is being updated, and `with_window` updated it again
  ("window not found"). Now deferred (`cx.defer`). Same in the palette.
- The undo dialog showed the typed name twice instead of the PROD notice.
- `kubyl_ui::format_keystroke` printed `secondary`; now ⌘ / Ctrl.
- Screenshot harness: macOS doesn't drive frames while the display sleeps or the window is
  occluded; `render_to_image` rasterized the stale startup frame. It now draws twice before
  capturing and takes steps `keys=<keystrokes>`, `wait=<ms>`, `action=<name>`,
  `palette=<query>` (e.g. `KUBYL_SCREENSHOT_ACTIONS="keys=: p o enter,wait=800"`).

**Gotchas.**
- A global `cx.on_action` handler runs inside the dispatching window's update: don't call
  `window.update` on it from there; `cx.defer` first (see `kubyl_palette::with_window`).
- The dialog binds `enter` itself; inputs inside dialogs need their own `enter` binding in a
  deeper context (`CommandPalette > Input`), else the dialog closes without confirming.
- `PaneEvent::Split` is handled after actions already queued: open views in a new split one
  `window.defer` later.
- GPUI gives context-free bindings the deepest precedence; `keys_for` mirrors that.
- Windows/Linux: `secondary-[`/`]` is `ctrl-[`/`]` there (alt-left/right also work);
  `secondary-p` is `ctrl-p` (inside the palette `ctrl-p` moves up). `:` and `?` bindings match
  through the typed character on every OS (`Keystroke::should_match`). Nothing else is
  platform-specific; only macOS was run interactively.

**API for later phases.**
- Open the palette: dispatch `kubyl_palette::Open { mode: Some(Mode::Resources), query: None }`
  (or `["palette::Open", {"mode": "actions", "query": "logs"}]` from a keymap);
  `kubyl_palette::open(mode, query, window, cx)` directly.
- Your actions appear in `>` automatically once registered with `ActionRegistry::register`;
  give them a context (`ActionSpec::bind(keys, Some("ResourceList && kind == Pod"))`) so they
  only show where they apply, and an availability predicate for the selection.
- Keymaps: add bindings in code as before; presets/users override them. Reference your actions
  by their GPUI name (`logs::ShowLogs`); add preset entries to `assets/keymaps/*.json` (the
  shell test `keymap_presets_name_existing_actions` checks they exist).
- Per-pane history is automatic for tabs with a `view_request`.

