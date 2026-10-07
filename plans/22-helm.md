# Phase 22: Helm charts, install, upgrade, rollback, uninstall

**Status:** done
**Depends on:** 04 (YAML editor, diff view), 12 (Helm releases: decoding, release tab), 05 optional (stream command output like a log)
**Owns:** `crates/kubyl_helm` (new), `crates/kubyl_helm_core` (new), `script/helm-dev.sh`; the release list and release tab move here from `kubyl_operators` (see "Shared-crate commits")
**Mockups:** board 20 · Helm, to be added to `design/mockups/generate.py` before the UI work: the Charts tab (repositories, search, a chart's details with versions, README and default values), the install dialog (name, namespace, version, values editor, preview), the upgrade review (values diff and per-object manifest diff), the rollback dialog (pick a revision, what changes), the uninstall confirmation (what gets deleted, what stays), the repositories editor, and the "helm isn't installed" state.

## Goal

Phase 12 made Helm releases visible but read-only: upgrade, rollback and uninstall were a "Copy
helm command" button. This phase makes Kubyl a Helm client: browse charts from the user's
repositories and OCI registries, install them, upgrade with a values editor and a preview of
what changes, roll back to a revision, and uninstall. Every write shows exactly what will
change first, and follows Kubyl's read-only and PROD rules.

Lens has the same (Charts view, Releases view with Upgrade/Rollback/Delete, repositories in
Preferences). Kubyl adds a real preview: rendered manifests diffed per object against the live
release before anything is applied.

## Decisions to make first

Each has a recommendation. Record the outcomes in the README's decision table.

1. **Engine.** Helm has no Rust implementation; its templates are Go templates with Sprig and
   Helm's own functions, and charts rely on their exact behavior. Recommended: run the user's
   `helm` CLI (3.13 or newer for `--dry-run=server`; 4.x supported) on the Tokio runtime,
   found through the login shell's `PATH` like exec plugins and agents. No `helm`, or an older
   one: Kubyl's Helm writes show the install command and stay read-only (phase 12's views keep
   working). Kubyl never downloads `helm` itself. The alternative, bundling a Helm build, means
   shipping and updating a Go binary per platform: not worth it.
2. **Credentials for `helm`.** `helm` reads the kubeconfig itself, which covers client
   certificates, tokens and exec plugins. Kubyl's own sign-ins (OIDC refresh, OpenShift OAuth)
   live in the keychain, which `helm` can't read. Recommended: pass `--kubeconfig <file>
   --kube-context <context>` always, and for contexts that sign in through Kubyl, add the
   current bearer token through the environment (`HELM_KUBETOKEN`, with `HELM_KUBEAPISERVER`
   and `HELM_KUBECAFILE`), never in arguments (visible in `ps`) and never in a file. Values go
   to `helm` on stdin (`-f -`), never through temp files. Nothing of a release's values or
   rendered Secrets is logged.
3. **Repositories.** Recommended: use Helm's own configuration (`helm env`:
   `HELM_REPOSITORY_CONFIG`, `HELM_REGISTRY_CONFIG`, `HELM_REPOSITORY_CACHE`), so the CLI and
   Kubyl see the same repositories. Adding a repository with a username and password stores it
   where `helm repo add` does (Helm's file, plain text: say so in the dialog and prefer
   `--pass-credentials`-free setups). OCI registries use `helm registry login` (Docker's
   credential store). Kubyl keeps no repository credentials of its own.
4. **Chart search.** Recommended: search the added repositories (`helm search repo --output
   json`, plus `helm show chart|readme|values` for details) and OCI references typed by the user.
   Artifact Hub search (`artifacthub.io` API) is opt-in in settings, off by default: Kubyl
   doesn't call third-party services unasked.
5. **Preview.** Recommended: every install, upgrade and rollback first renders with
   `helm … --dry-run=server --output json` (server-side lookups and validation; it needs
   `get` access to look things up), then shows:
   - for install: the objects that will be created, grouped by kind, with Secrets masked;
   - for upgrade and rollback: a per-object diff of the rendered manifest against the current
     release's manifest (phase 12 already decodes it) in phase 04's diff view, plus the values
     diff;
   - hooks that will run, and the CRDs a chart installs (which Helm never upgrades).
   Applying runs the same command without `--dry-run`. A dry run that fails shows Helm's error
   and nothing is applied.
6. **Crates.** Recommended: new `kubyl_helm` / `kubyl_helm_core`, and move phase 12's Helm code
   (`helm::decode`, `helm::present`, `helm::service`, the Helm releases sub-tab and the release
   tab) from `kubyl_operators` into them in a first, behavior-preserving commit. Helm isn't OLM,
   and the write side would otherwise make `kubyl_operators` the Helm crate anyway. The
   Operators tab keeps a "Helm Releases" sub-tab that renders `kubyl_helm`'s view.

## Tasks

### Shared-crate commits (each lands on its own, first)
- [x] Move phase 12's Helm modules and views from `kubyl_operators(_core)` to `kubyl_helm(_core)`; `kubyl_operators` re-exports what it still needs. No behavior change; phase 12's tests move along and pass
- [x] `kubyl_resources_core::errors`: the API error messages phase 12 kept in `kubyl_operators_core`, shared by the Helm and OLM core crates (code review)
- [x] `kubyl_kube_core`: a "how to run a CLI against this context" helper shared with the agents' commands: kubeconfig path, context name, and the bearer-token environment for Kubyl-signed-in contexts (decision 2)

### The `helm` CLI (`kubyl_helm_core::cli`)
- [x] Find `helm` (login shell `PATH`, `helm.path` in settings), probe `helm version --short`, require 3.13+; read `helm env` (its `KEY="value"` lines: Helm 3 and 4 print no JSON for it)
- [x] Run commands on Tokio with a timeout and cancellation, values on stdin, JSON output parsed (`--output json` where Helm offers it), stderr kept for the error message (scrubbed of token shapes)
- [x] Map Helm's common errors to messages: missing RBAC verb, "cannot re-use a name that is still in use", "another operation (install/upgrade/rollback) is in progress" (offer to show the stuck release's status), timeouts
- [x] A fake `helm` for tests (a script that records args, env and stdin and prints recorded JSON), so the core's tests never need the real CLI

### Repositories and charts
- [x] Repositories editor (Helm tab and settings): list, add (HTTP repo with optional basic auth, CA, client certificate; OCI registry login), update (`helm repo update`), remove
- [x] Charts tab: search across repositories (name, description, latest version, app version, repository), with filters by repository; a chart's details: versions, `Chart.yaml` metadata, README (markdown view), default values (read-only YAML)
- [x] OCI references (`oci://registry/path/chart:version`) typed into the search
- [x] Artifact Hub search behind `helm.artifact_hub: true` (decision 4)

### Install
- [x] Install dialog: release name (validated), namespace (existing or new, `--create-namespace`), chart version, values editor (phase 04, YAML with the chart's `values.schema.json` when present, starting from the chart's defaults or empty "override only" mode), options (wait, timeout, atomic/rollback-on-failure, skip CRDs, description)
- [x] Preview (decision 5), then install; progress and Helm's output in the dialog; on success open the new release's tab
- [x] Read-only clusters: hidden; PROD: typed confirmation

### Upgrade
- [x] From a release (tab, list row, palette): pick the chart version (from the release's repository when known, else type a reference), values: current user-supplied values (`helm get values`) in the editor, with "reset to chart defaults" and "reuse values" choices
- [x] Preview: values diff, per-object manifest diff, objects added and removed, CRD changes Helm won't apply (warning), hooks
- [x] Upgrade with the same options as install; show the new revision

### Rollback and uninstall
- [x] Rollback: pick a revision from the release history (phase 12), preview the manifest diff from the current revision to it, roll back (options: wait, timeout, cleanup on fail, no hooks)
- [x] Uninstall: show the release's objects that will be deleted, and what stays (PVCs from StatefulSet templates, CRDs, objects with `helm.sh/resource-policy: keep`); options: keep history, no hooks, wait; typed confirmation on PROD
- [x] Replace phase 12's "Copy helm command" buttons with these actions (copying the command stays as a secondary action)

### Across Kubyl
- [x] Palette: `Helm: Install Chart…`, `Helm: Upgrade Release…`, `Helm: Roll Back Release…`, `Helm: Uninstall Release…`, `Helm: Repositories…`, `:charts`
- [x] Objects managed by a release (`app.kubernetes.io/managed-by: Helm` and `meta.helm.sh/release-name`) show "Managed by Helm release X" in their details with a link, and editing them warns that the next upgrade will revert the change
- [x] "Ask agent" on a release (phase 21's `AskAgent`): chart, versions, status, last revisions, failing resources; never values
- [x] Settings: `helm.path`, `helm.artifact_hub`, `helm.default_timeout`, `helm.atomic` (default true)

### Dev setup and tests
- [x] `script/helm-dev.sh`: a local HTTP chart repository (two versions of a small chart, one with a values schema and a hook, one that fails its hook), an OCI registry on kind's registry with a chart, and a release stuck in `pending-upgrade`
- [x] Core tests with the fake `helm`: arguments, environment (no token in args), stdin values, error mapping
- [x] Live tests (ignored) with the real `helm` against `kubyl-dev`: install, upgrade with a values change, rollback, uninstall, OCI install
- [x] GPUI tests: install dialog validation, preview rendering, read-only clusters hide the actions

## Acceptance criteria

- With `helm` 3.13+ installed: install a chart from the dev repository into a new namespace with
  edited values, see the preview first, and the release appears with its resources.
- Upgrading shows a per-object diff and a values diff before applying; rolling back shows the
  diff to the chosen revision; uninstalling lists what will be deleted and what stays.
- No token appears in `helm`'s arguments or in any file; values never touch the disk.
- Without `helm`: the Helm views work as in phase 12 and the actions show how to install it.
- Read-only clusters show no Helm write action; PROD needs typed confirmation.

## Risks

- **`helm` versions.** Flags and JSON output change between 3.x and 4.x. Probe the version and
  test both in CI with the fake and locally with real binaries.
- **Long or stuck operations.** `--wait` can take minutes; a release can be left
  `pending-*`. Show progress, allow cancelling (Helm then reports the state), and explain how to
  recover a stuck release.
- **Server-side dry run needs permissions** the user may lack (lookups, CRD validation): fall
  back to `--dry-run=client` with a note that the preview is less exact.
- **Secrets in rendered manifests** (charts that render Secrets from values): masked in previews
  and diffs like everywhere else.

## Later (not in this phase)

- Installing metrics-server or kube-prometheus-stack from Kubyl's empty states (phase 07) with
  this flow.
- Helm plugins, post-renderers, `helm test`, and diffing a release against live objects (drift).
- A native renderer, if a maintained Rust implementation of Helm templates ever exists.

## Handoff log

### 2026-10-07 (branch `phase/22-helm`, uncommitted for review)

**Shipped.**
- Mockups first: board 20 · Helm in `design/mockups/generate.py` (8 frames: Charts tab, install
  dialog, install preview, upgrade review, rollback, uninstall, repositories, "helm isn't
  installed" with a stuck release and a running upgrade).
- Shared-crate moves (behavior-preserving): phase 12's `helm::{decode, present, release,
  service}` moved to `kubyl_helm_core` and `errors` to `kubyl_resources_core` (first to
  `kubyl_helm_core`; moved again in the code review); the `Helm` entity, the release tab and the
  release list (now a standalone `kubyl_helm::releases::ReleasesView`) to `kubyl_helm`.
  `kubyl_operators(_core)` re-export the old paths (`kubyl_operators::helm::…`,
  `kubyl_operators::release`, `kubyl_operators_core::{errors, helm}`), so `kubyl_updates` didn't
  change; the Operators tab embeds `ReleasesView` on its Helm sub-tab (j/k/enter/`/` forwarded) and
  got a "Browse charts" button there. `kubyl_kube_core::cli::CliTarget` +
  `ManagerCore::cli_target(id)` (kubeconfig, context, server, and the OIDC/OpenShift sign-in whose
  token goes into the child's environment); the agent's commands use it for their kubeconfig and
  context (never the token). `kubyl_kube` re-exports `cli`; `ConnectionManager::install` is now
  `pub` (other crates' GPUI tests need it, as the AGENTS.md gotcha already said).
- `kubyl_helm_core`: `cli` (resolve/probe/run: login-shell `PATH`, 3.13+, `helm env`, timeout,
  SIGINT cancel, writes survive Kubyl quitting (true only since the code review fixes below), values/passwords on stdin, token only in
  `HELM_KUBETOKEN`, stderr scrubbed, error mapping: RBAC verb, name in use (Helm 3 "re-use" and
  Helm 4 "reuse"), operation in progress, timeouts, schema, not found), `cmd` (install, upgrade,
  rollback, uninstall, get values, status; `--atomic` vs Helm 4 `--rollback-on-failure`),
  `repo` (chart references incl. OCI with `--plain-http` for localhost, repository
  list/add/update/remove, registry login, search, versions, chart details via `helm pull
  --untar`, Artifact Hub), `preview` (per-object diffs with Secrets masked, install groups,
  hooks, CRDs, rollback from stored revisions, uninstall plan), `values` (parse, overrides vs
  defaults, JSON Schema check through phase 04's validator, masked diffs), `agent` (release text,
  workloads that aren't ready), `settings` (`helm.path`, `helm.artifact_hub`,
  `helm.default_timeout`, `helm.atomic`), `release::managed_by`.
- `kubyl_helm`: `HelmCli` (probe state, re-probes on `helm.path` changes and "Check again"),
  `HelmOps` (running writes with Helm's output and Cancel; they outlive the dialog), the Charts tab
  (`helm_charts`: repositories filter, search, OCI references, Artifact Hub when enabled, details
  with versions, Chart.yaml metadata, README as markdown, default values, CRDs), dialogs (install
  with the values editor and schema, preview, run → opens the new release's tab; upgrade with
  edit/reuse/reset values and the review; rollback; uninstall; repositories with HTTP basic auth,
  CA, client cert, OCI login; "helm isn't installed"; a release picker for the palette), the
  release tab and list actions (Upgrade `u`, Roll back `b`, Uninstall `⌃d`, Install chart `i`,
  Ask agent `⇧a`, Copy helm command stays as a secondary action), the stuck-release banner with
  "Roll back to N…", "Managed by Helm release X" in details and the YAML editor's warning,
  palette entries `Helm: Install Chart…`, `Upgrade Release…`, `Roll Back Release…`,
  `Uninstall Release…`, `Repositories…`, `Charts` (`:charts` finds it).
- `script/helm-dev.sh` (HTTP repo, OCI registry, releases incl. one stuck in `pending-upgrade`,
  `--stuck`, `--delete`); AGENTS.md lists it and the live tests.

**Decisions** (README table): Helm engine, Helm credentials, Helm repositories and charts, Helm
previews, Helm crates (all as the plan recommended), and the phase 12 "Helm releases" row updated.

**Verified.** `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo deny
check`, `script/check-core-crates.sh`, `cargo test --workspace`. New tests: `kubyl_helm_core` 34
unit tests + 4 fake-`helm` tests (`tests/fake_helm.rs`: args, env without the token in args,
stdin values and passwords, error mapping, timeout and cancel), `kubyl_helm` 8 (4 GPUI: install
dialog validation, preview rendering, read-only clusters get no dialog and no write hints, no
`helm` opens the install note), `kubyl_kube_core` 1. Live (`crates/kubyl_helm_core/tests/live.rs`,
4 tests, all passing) against `kubyl-dev` with helm 4.3.0 **and** helm 3.19.0: install with
preview (nothing applied by the dry run), upgrade to 0.2.0 with a values change (per-object diff,
values diff, CRD warning, hooks), rollback (preview from stored revisions), uninstall, OCI install
from the local registry, schema rejection, name in use, the stuck release's "in progress" error,
repositories and versions. In the app (`design/screenshots/phase-22-*.png`): Charts tab, install
dialog with schema, server-side preview, install running (release created), release tab, upgrade
review (manifest and masked values diff), rollback, uninstall, repositories, missing `helm`,
stuck release banner, Operators' Helm sub-tab, "Managed by Helm release" in details and the YAML
editor's notice.

**Deviations from board 20.** The upgrade dialog's footer shows the summary instead of a typed
name on non-PROD clusters (PROD shows the typed name next to the buttons, like the board). The
install preview's object list has group headers per kind and a notes section under hooks/CRDs.
The values editors show values unmasked (they're editors; masking applies to views and diffs).

**Notes for later phases / gotchas.**
- `helm search repo --regexp` matches descriptions too: filter versions by exact `repo/name`
  (`repo::parse_versions`).
- Helm 4 needs `--plain-http` for HTTP registries, also on localhost; Kubyl adds it only for
  localhost/127.0.0.1 references. Other plain-HTTP registries aren't supported from the UI.
- `--rollback-on-failure`/`--atomic` imply `--wait`: a PVC that never binds (kind's local-path
  class waits for a consumer) times the operation out; the dev chart mounts its PVC for that.
- Escape in a gpui-component dialog's editor closes the dialog (screenshot steps must avoid it).
- A release's chart repository isn't recorded by Helm: upgrades look the chart up by name in the
  added repositories (a picker when several have it) or take a typed reference.
- Not done (out of scope / later): progress beyond Helm's stderr (Helm prints little without
  `--debug`, which would print rendered manifests), plain-HTTP registries other than localhost,
  a settings UI page (Kubyl has none; the repositories editor is in the Charts tab and the palette).

### 2026-10-07 · Code review fixes (branch `phase/22-helm`, uncommitted)

Each finding was checked against the code and, where it mattered, against real `helm` 4.3.0 and
3.19.0.

**Major**
1. *Writes die when Kubyl quits:* **fixed**, differently than suggested. Verified that Go dies of
   SIGPIPE writing to fd 1/2 after the reader is gone even when SIGPIPE is inherited as ignored
   (`dieFromSignal` resets it), with both helm binaries, so ignoring SIGPIPE in `pre_exec` doesn't
   help. Now a write's `helm` (Unix) inherits the read end of its own stderr pipe (cleared
   close-on-exec in `pre_exec`): the pipe always has a reader and Helm's warnings never kill it.
   stdout stays a plain pipe (Helm prints the release JSON only after recording it; keeping it
   alive could block forever on a large JSON). Writes also run in their own process group, so a
   terminal Ctrl-C doesn't reach them. Tests: `fake_helm::writes_outlive_kubyl_and_reads_do_not`
   (fails without the fix) and live `writes_finish_after_kubyl_goes_away` (runtime dropped 1.5 s
   into an `install --wait`; the release ends `deployed`, helm 3 and 4).
2. *"(latest)" can install another version:* **fixed.** Versions are always explicit: install
   defaults to the newest non-pre-release (what `helm install` picks), labelled "(latest)",
   pre-releases "(pre-release)" (`repo::{is_prerelease, latest_stable}`,
   `dialogs::version_label`); upgrade defaults to the installed version (a values-only upgrade no
   longer moves to the newest chart); the Charts tab's menu picks explicit versions too.
3. *Apply rebuilds the command:* **fixed.** `Step::Preview` keeps the exact spec and values the
   dry run ran; Install/Upgrade apply those with `--version` pinned to the chart version the dry
   run rendered (`preview::rendered_version`).
4. *Stateful entities:* **fixed.** `kubyl_helm_core::ops::HelmOpsCore` and
   `kubyl_helm_core::cli::HelmCliCore` are plain services on `kubyl_base::Host`;
   `kubyl_helm::{ops::HelmOps, cli::HelmCli}` host them with `hosted` (Deref to the core). Core
   tests on `TestHost` (lines and revision, cancel with a grandchild holding the pipes, re-probe on
   `helm.path` changes). The dry run with its client-side fallback moved to
   `preview::{install_dry_run, upgrade_dry_run}`.

**Minor**
5. *Env scrub:* **fixed.** Every `HELM_KUBE*`, `HELM_NAMESPACE` and `HELM_DRIVER` of the user's
   environment is removed; `HELM_DRIVER` is set per command (`Invocation::driver`: `secret` for
   installs, the release's driver for upgrade, rollback, uninstall, get values, status; the
   specs carry `driver`). Test: `tests/fake_helm_env.rs` (own binary: it sets process env).
6. *Timeout parsing:* **fixed.** `parse_duration` follows Go's `time.ParseDuration` (unit
   required, `ns`…`h`, `0`), rejects overflow (`try_from_secs_f64`, Go's ~292-year limit);
   `HelmSettings::timeout()` falls back to `5m0s` for an invalid `helm.default_timeout`.
7. *Operation stuck "Running":* **fixed.** Readers fill shared buffers; after exit, cancel or
   timeout each gets at most 3 s, then it's aborted (dropping the progress sender), on the success
   path too. `HelmOpsCore` finishes from the run's result, independent of the line stream.
   Non-Unix: no SIGINT exists, so `helm` is killed right away instead of after a pointless 15 s.
8. *Preview Debug:* **fixed.** Manual `Debug` for `Preview` (counts only) and for
   `preview::Document` (its text holds rendered Secrets).
9. *Secret data change missed:* **fixed.** `data`/`stringData` are compared raw for every
   Secret, whatever else changed.
10. *Install on read-only Charts tab:* **fixed.** Button hidden, `InstallSelected` ignored,
    "Install…" added to `WRITE_HINTS` and the Charts tab filters its hints
    (`design/screenshots/phase-22-charts-read-only.png`).
11. *Hooks listed with "No hooks":* **fixed** in uninstall and the rollback preview
    (`PreviewPane::set_hooks_skipped`).
12. *Namespace keys:* **fixed.** A `metadata.namespace` equal to the release's counts as none
    (`documents`/`object_changes` take the release namespace); other namespaces stay distinct.
13. *Leading `-` in positional args:* **fixed.** `ChartRef::validate` (install/upgrade specs,
    typed references, Artifact Hub hits), `validate_repository_name`, `validate_registry_host`;
    `--` before positional args of `search repo`, `repo add/update/remove`, `registry login`
    (verified that helm 3 and 4 accept `--` there).
14. *errors in the Helm core crate:* **fixed.** Moved to `kubyl_resources_core::errors`;
    `kubyl_operators_core::errors` re-exports it.

**Nits**
- Helm 2: **fixed** (case-insensitive "tiller" in Helm's output; `Client: v2…` parses as 2).
- Dead code: **fixed** (`preview::grouped`, `CliTarget::needs_token`, `cli::writable` removed).
- Interrupt comment: **fixed** (see 7).
- Plan wording `helm env --output json`: **fixed** (Helm has no JSON for `env`).
- Repositories: **fixed.** Adding a name that exists asks for a second click before
  `--force-update` replaces it; Remove asks for a second click; the password field is cleared
  after every attempt (it went to `helm` either way).
- Helm 4 registry login to localhost: **fixed** (`--plain-http` for Helm 4 and a local registry;
  verified: helm 4.3 fails without it, helm 3.19 logs in without it). `[::1]` without a port:
  **fixed**.
- `--create-namespace` when the namespace list is unknown: **kept off**, with a note under the
  checkbox. Defaulting it on would break installs into existing namespaces for users who can't
  create namespaces (Helm's create gets a 403 before AlreadyExists), which are the users who
  can't list them.
- "In progress" offers the release's status: **fixed** ("Show the release's status" next to the
  stuck-release note of a failed, cancelled or timed-out operation opens the release's tab).

**Test gaps:** all **covered**: scrubbed `HELM_KUBE*` (`fake_helm_env`), server→client fallback
(`fake_helm::previews_fall_back_to_a_client_side_dry_run`), a write not killed when its future is
dropped (`fake_helm::writes_outlive_kubyl_and_reads_do_not`, live
`writes_finish_after_kubyl_goes_away`), the Kubyl sign-in token only in the environment
(`fake_helm::kubyl_sign_ins_hand_helm_the_token_in_its_environment`, through
`CliTarget::with_sign_in` and an OpenShift sign-in on an empty credential store).

**Also found:** the preview pane's object body was `h_full` under the tabs row, so long manifests
ran 34 px into the dialog's footer (now `min_h_0`, clipped). Screenshots retaken:
`phase-22-install-dialog.png` (explicit "0.2.0 (latest)"), `phase-22-upgrade-review.png` (stays
on the installed 0.1.0; the old one said "→ latest"), new `phase-22-charts-read-only.png`.

**Verified.** `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test
--workspace` (1134 passed), `cargo deny check`, `script/check-core-crates.sh`. `kubyl_helm_core`:
40 unit + 8 fake-`helm` + 1 env tests; `kubyl_helm`: 9. Live (`tests/live.rs`, now 5) pass with
helm 4.3.0 and 3.19.0 against `kubyl-dev`.

### 2026-10-07 · Second review (branch `phase/22-helm`)

An independent review of the whole change set (after the code review fixes above), against
`main` with the new credentials API (`kubyl_kube_core::auth::Credentials`, scoped handles).
Each finding was checked against the code first.

**Fixed**
1. *Scoped credential handles fell back to the kubeconfig's tokens (security):* `cli::run`
   dropped a failed `CliTarget::token()` and ran `helm` anyway, which then used the tokens
   written into the kubeconfig (an OIDC refresh/id token, an `oc login` token): with a scoped
   handle that's acting as the kubeconfig's author. `ManagerCore::cli_target` now records the
   manager's handle (`CliTarget::with_credentials(&credentials, kubyl_signs_in)`); for OIDC and
   OpenShift contexts of a scoped handle a missing or failed sign-in is `SignInRequired`
   (`CliTarget::token`, also when the manager has no sign-in object at all) and `helm` doesn't
   run (`ErrorKind::SignInRequired`). The default handle keeps phase 22's behavior. The token
   itself already came from the manager's handle (`oidc_auth`/`openshift_auth` use it). Tests:
   `kubyl_kube_core::cli::tests::scoped_handles_require_kubyls_own_sign_in`,
   `fake_helm::scoped_handles_without_a_sign_in_never_run_helm_on_the_kubeconfigs_tokens`.
2. *Install sent the whole defaults file as values while the chart's details loaded or after
   they failed:* the editor still held the previous version's defaults and `payload` sent the
   text as is, pinning every default as a user-supplied value (the install preview has no values
   diff to show it). Defaults mode now needs the picked version's details (`blocker` says "Wait
   for the chart's default values", or to use override-only values when the pull failed);
   switching to override-only without details clears the editor. GPUI test extended (blocked
   until details, payload is only the changed keys).
3. *Typing in any install field reset `--create-namespace`:* every input re-ran
   `sync_namespace`, so a box ticked by hand (namespaces not listable) was unticked by typing a
   description. Only the namespace input and menu re-sync it now.
4. *Upgrade applied stale async results:* versions and details loads were pushed into a growing
   `Vec<Task>` and never cancelled, so an older version's schema could land after a newer pick.
   One task each now (a new load drops the old one); the schema is cleared when the chart
   changes or a pull fails. The unread `details` field is gone.
5. *Upgrade guessed the repository when several have the chart:* the first was preselected.
   With several candidates the user picks ("Pick the repository the release comes from…"); one
   candidate is still used directly.
6. *Stale dialogs:* upgrade, rollback and uninstall check at apply time that the release's latest
   revision is still the one the preview was made from (`dialogs::release_changed`); if another
   revision landed, they say so instead of applying.
7. *PROD confirmation survived Back:* the typed name is cleared when a new install preview or
   upgrade review arrives.
8. *Copied commands ignored the storage driver:* `present::commands` takes the release's driver
   and prefixes `HELM_DRIVER=configmap` for ConfigMap-stored releases (test).
9. *A failed chart pull in the install dialog couldn't be retried* (the key stayed set): cleared
   on failure.
10. *`helm`'s stderr read with `lines()`:* a line that isn't UTF-8 ended the reader, after which a
    read's `helm` dies on the closed pipe and a write's blocks once the pipe fills. Read as bytes
    (lossy) now.
11. *Artifact Hub hits of an earlier query* stayed listed under a shorter or `oci://` query: cleared
    when the query isn't searched.
12. *Dev script and live tests could adopt a namespace they didn't make:* `helm-dev.sh` labelled an
    existing unmarked `kubyl-helm` (which `--delete` then removed); it now stops instead. The live
    tests assert `kubyl-helm-live` carries their label before installing into it.
13. *Leaks and dead code:* the "helm isn't installed" dialog's observer was detached (one per
    open, forever); it's the dialog's subscription now. Removed the unused `RepositoriesChanged`
    event and `dialogs::preview::pane`; `dialogs::read_only` re-uses `cli::read_only`;
    `ops::start` and the dialogs' apply functions lost their unused `window` parameter; the agent
    module's tests moved to the end of the file. Repository URLs in the repositories dialog are
    shown without `user:password@`.
14. *Live tests run back to back failed:* the previous run's cleanup was still deleting
    `kubyl-helm-live`; the tests now wait for it to go before creating it again.

**Skipped (with reasons)**
- *Flag injection through release names from the cluster:* not possible. Release names come
  from the storage objects' `name` label, and label values must start with a letter or digit;
  install names are validated, chart references, repositories and registries were fixed in the
  first review.
- *`HELM_KUBECAFILE` (decision 2):* not needed. `helm` gets `--kubeconfig` and reads the CA from
  it; only the token isn't in the file.
- *The agent's `kubectl` (phase 21) uses `cli_target` for the kubeconfig and context only:* with
  a scoped handle it would sign in with the kubeconfig's own credentials. Agents run in the
  desktop app (the default handle); a multi-user host needs the same treatment there (phase 21's
  crates, not this phase's).
- *Unbounded stderr buffer:* Helm prints a few lines without `--debug` (which Kubyl never sets).

**Verified.** `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test
--workspace`, `cargo deny check`, `script/check-core-crates.sh`. New tests: `kubyl_kube_core` 1
(scoped handles), `kubyl_helm_core` 1 fake-`helm` test (now 9) and the driver prefix of copied
commands, `kubyl_helm`'s install dialog test extended. Live (`tests/live.rs`, 5) pass with helm
3.19.0 and 4.3.0 run back to back against `kubyl-dev`; `helm-dev.sh --stuck` re-run. The upgrade
dialog re-checked in the app (one repository has the chart: picked as before).
