# Phase 11: Kubeconfig editor

**Status:** done (branch `phase/11-kubeconfig-editor`; see the handoff log)
**Depends on:** 01 (sources, auth, client building, file watching), 04 (YAML editor, schema validation and diff for the raw view and the save preview)
**Owns:** `crates/kubyl_kubeconfig` (new)
**Mockups:** board 11 · Kubeconfig editor in `design/mockups/generate.py` (editor tab, YAML tab and "Test connection" results, "New kubeconfig" wizard, save preview and exec consent).

## Goal

Create and edit kubeconfigs in a GUI instead of a text editor: clusters (servers, CAs), users
(credentials of every kind), contexts. Test a connection before saving, and save without breaking
the file for `kubectl` and the other tools that share it.

Phase 01 covers the sources themselves: adding files, folders and pasted YAML, removing sources,
turning `~/.kube/config` and `$KUBECONFIG` on and off, deleting pasted kubeconfigs (fixed
2026-09-25 on `phase/01-kubeconfig-sources`). This phase edits what's inside them.

## Decisions to make first

Each has a recommendation; record the outcome in the README's decision table, since the first
one changes the current rule "Kubyl never modifies kubeconfig files".

1. **Which files get written.** Recommended: Kubyl-owned kubeconfigs (pasted or created in Kubyl,
   under `<config dir>/kubeconfigs/`) are edited freely. Other files (`~/.kube/config`,
   user-added) only after a per-file opt-in ("Edit this file"), and then always with a diff
   preview, a backup, an atomic write and a changed-on-disk check. The alternative for users who
   never want Kubyl to touch their files: "Save as a Kubyl copy" (the copy replaces the original
   as a source).
2. **Comments and key order.** A serde round trip drops comments and reorders keys, which is a
   bad surprise in a hand-maintained `~/.kube/config`. Recommended: spike a comment-preserving
   edit (a YAML CST/document-editing crate, or targeted text edits of the changed nodes). If
   that isn't solid, keep the serde writer and make the loss visible in the save preview
   ("3 comments will be removed").
3. **Where credentials live.** Inline in the file, as kubectl expects (mode 0600), is the
   default. Optional per user entry: keep the secret in the OS keychain and write an `exec`
   credential plugin that is Kubyl itself (`kubyl credential <id>`), so the file holds no secret
   and kubectl keeps working while Kubyl is installed. Recommended: inline first, keychain as
   a follow-up.

## Tasks

### Model and files
- [x] Kubeconfig document model: clusters, users, contexts, `current-context`, preferences and extensions. Unknown fields survive a round trip
- [x] Load and save per file: atomic write (temp file + rename), mode 0600 for new files and for files with inline credentials (otherwise keep the existing mode), timestamped backups (`<config dir>/kubeconfig-backups/`, keep the last N), refuse to overwrite a file whose hash changed since it was loaded
- [x] Changed on disk while editing (phase 01's watcher): a banner with "Reload" and "Keep mine" (shows the diff)
- [x] Validation as you type: contexts pointing at missing clusters or users, duplicate names, invalid server URLs, unreadable certificate or key files, client certificates and CAs that are expired or expire soon (parse the PEM, show NotAfter), `insecure-skip-tls-verify` together with CA data
- [x] `$KUBECONFIG` merges several files: the editor edits one file at a time, and the merged view offers "open the file that defines this"

### Editor
- [x] Opens from the Clusters & kubeconfigs tab (source row "Edit…", context row "Edit context…"), the Explorer "+" menu ("New kubeconfig…") and the palette
- [x] Layout: a list on the left grouped into Contexts, Clusters and Users (add, duplicate, rename, delete; deleting a cluster or user that contexts still use asks first), a form on the right, and a "YAML" tab with the raw file (phase 04 editor with the kubeconfig schema), kept in sync with the form
- [x] Cluster form: server URL, CA (system trust store, file, pasted PEM, or fetched from the server with the fingerprint shown for confirmation), TLS server name, proxy URL, insecure skip verify (with a red warning), disable compression
- [x] User form by auth type: token (masked, reveal on click), token file, client certificate and key (file or inline PEM; shows subject, issuer, expiry), exec plugin (command, args, env, apiVersion, interactive mode, provide cluster info, install hint) with presets for AWS EKS, GKE (`gke-gcloud-auth-plugin`), AKS (`kubelogin`) and kubelogin OIDC, OIDC auth provider (issuer, client ID and secret, scopes, extra parameters), basic auth (marked deprecated)
- [x] Context form: cluster and user pickers, namespace (a live list once a test succeeded). Kubyl's own overrides (display name, color, production, read-only) stay in settings.json and are shown next to it
- [x] Secrets are masked by default, never logged, never written to `state.json`; copying one is an explicit action

### Verify and test
- [x] "Test connection" for a context, from the in-memory edits, without saving (phase 01's client builder). Before it runs an exec plugin that comes from an import or unsaved edits, it shows the command and arguments and asks for consent (`interactiveMode` isn't consent). Steps, each with ✓/✗ and a plain-language error: DNS and TCP, TLS (handshake, CA verification, server certificate details), `/version`, authentication (`SelfSubjectReview`: user name and groups), permissions (can list namespaces, can list pods, cluster-admin or not), latency
- [x] Failures suggest the fix: "certificate signed by unknown authority: fetch the CA from the server?", "token expired", "`aws` isn't installed: see the install hint"
- [x] After consent, exec plugins run with the login-shell `PATH` (phase 01) and show their stderr. OIDC runs the sign-in flow
- [x] "Test all contexts" of a file, results shown in the list

### Creating and sharing
- [x] "New kubeconfig…" wizard: name, cluster (server, CA), credentials, context, test, save (Kubyl-owned by default, or pick a path)
- [x] From a service account on a connected cluster: create or pick a ServiceAccount, get a token (TokenRequest with an expiry, or a token Secret), optionally bind a Role or ClusterRole, generate the kubeconfig
- [x] From cloud CLIs when they're installed: EKS (`aws eks describe-cluster`), GKE (`gcloud container clusters describe`), AKS (`az aks show`), producing the same entries their `get-credentials` commands write
- [x] Export one context as a standalone kubeconfig, with or without credentials (warns when they're included); copy as YAML
- [x] Move or copy contexts between files, merge files, split a file into one file per context

### Integration
- [x] After saving, phase 01 reloads the file; edited contexts reconnect and their clients are rebuilt
- [x] Palette: "Kubeconfig: New…", "Kubeconfig: Edit Current Context", "Kubeconfig: Test Current Context"
- [x] Settings: number of backups kept, whether external files may be edited at all

## Acceptance criteria

- Create a kubeconfig for the kind dev cluster from scratch in the wizard: server URL, CA fetched from the server after confirming its fingerprint, client certificate and key from files. The test passes (TLS, authenticated as `kubernetes-admin`, can list pods), and after saving the context shows up in the sidebar and connects.
- Opt in to editing `~/.kube/config` and change a context's namespace: the save preview shows only that change (comments kept, or their loss shown), a backup exists, and `kubectl` still works with the file.
- A wrong CA, an expired token and a missing exec plugin each fail the test at the right step, with a clear message.
- No secret appears in logs, `state.json` or crash reports, and secrets are masked in the UI.

## Risks

- Comment-preserving YAML editing is immature in Rust. Losing comments in a hand-edited `~/.kube/config` would cost trust: spike it first (decision 2).
- Other tools write the same files (`aws eks update-kubeconfig`, `gcloud`, `kubectl config`). The hash check and atomic rename prevent lost updates, but the UI has to explain the conflict well.
- Fetching a CA from the server is trust on first use: always show the fingerprint and make confirming it explicit.
- Cloud CLI helpers depend on installed CLIs and their versions; keep them optional.

## Handoff log

### 2026-09-26 (branch `phase/11-kubeconfig-editor`)

Everything in the task list is done; the deferrals are at the end. The stub-crate PR was
skipped on request: the crate, the small commits in crates it doesn't own (`kubyl_core`,
`kubyl_kube`, `kubyl`) and the feature work are one PR. Board 11 is in
`design/mockups/generate.py`; **the published mockup artifact still needs board 11 (and
boards 12–15 from phase 10)**. Screenshots: `design/screenshots/phase-11-*.png` (form, YAML
tab, test passed and failed, wizard with a fetched CA, save preview, exec consent).

**Decisions** (README decision table: "Kubeconfig writes", "Kubeconfig comments and key
order", "Kubeconfig credentials", "Exec-plugin consent and CA trust").
- Kubyl-owned files (`<config dir>/kubeconfigs/`) are edited freely; others only after "Edit
  this file" (per file, `kubeconfig_editor.editable_files`), or saved as a Kubyl copy that
  replaces the original as a source. `kubeconfig_editor.allow_external_edits: false` removes
  the opt-in; `backups_kept` (default 10) bounds the backups per file.
- Comments and key order are kept by our own writer (`yaml::write`): the spike found
  `yamlpatch` and `yaml-edit` unsafe for `~/.kube/config` (details in the README). The writer
  edits spans of `kubyl_yaml::parse`, reparses the result and must get the edited model back;
  otherwise it renders the file from scratch and the save preview lists every comment that
  would be lost. A test edits every field of every entry kind and checks each one is in place.
- Credentials stay inline (0600). The keychain-backed `kubyl credential <id>` plugin is a
  follow-up.

**Files** (`files`). Saves write a temp file in the same folder, fsync, rename (symlinks are
followed to the target), and refuse when the file's SHA-256 changed since it was loaded
(`SaveError::Changed`). Backups: `<config dir>/kubeconfig-backups/<stem>-<path hash
8>-<YYYYMMDD-HHMMSS>[-n].yaml`, 0600, the last N per file. New files and files with inline
credentials are 0600, others keep their mode. The editor checks the file's hash every 2 s
(also for files that aren't sources): unchanged edits reload silently; with unsaved edits a
banner offers "Show diff", "Reload" and "Keep mine" (the next save applies the edits to the
new text, keeping its comments).

**Editor** (`editor`, `editor_ui`, `forms`, `yaml_tab`). A tab per file (`ViewRequest::for_path`
with the `kubeconfig` view kind; restored after restart by path, never with content). List of
contexts, clusters and users (filter, add, duplicate, rename with references and Kubyl
overrides moved, delete with "used by" confirmation, test badges), the form (cluster: CA from
system, file, pasted PEM or fetched with fingerprint, cert details and expiry, TLS name,
proxy, insecure with red notice, compression; user: token with JWT expiry, token file, client
cert and key as files or inline, exec with EKS/GKE/AKS/kubelogin presets, OIDC provider,
basic auth marked deprecated, other providers kept; context: cluster/user pickers, namespace
with the list from the last test, current context, Kubyl overrides from settings.json), the
context's problems (its cluster's and user's too) and last test, and a YAML tab (phase 04's
editor with the kubeconfig OpenAPI schema from `schema`, hover, completion of names; secrets
shown as `••••••••` until revealed and restored when parsing). The toolbar counts changed
lines. Opens from the Clusters tab (source row edit button, "Edit context…" in the connection
card, "New…"), the Explorer "+" menu and the palette ("Kubeconfig: New…", "Edit Current
Context", "Test Current Context", "New from Service Account…", "Import from Cloud CLI…").
`$KUBECONFIG` merges: the editor edits one file; "Edit context…" opens the file that defines
the context.

**Connection test** (`conntest`, `tls`, `certs`, `panel`). Runs from the in-memory document
(unsaved edits included) on the kube runtime, steps DNS and TCP → TLS → Credentials → API
server (`/version`) → Authentication (`SelfSubjectReview`) → Permissions (SSAR: list
namespaces, list pods in the context's namespace, cluster-admin) → Latency (median of 3).
The first failure stops the rest with the reason. TLS is checked with our own rustls
handshake before any credential is loaded: a failure says why ("signed by unknown authority:
the server's CA isn't the one this kubeconfig trusts", expired, name mismatch…), offers "Fetch
the CA from the server…" and skips the rest with "no credentials were sent"; there is no
insecure fallback. Fixes offered: fetch CA, install hint for a missing exec command, sign in
(OIDC and kubelogin-style exec, through phase 01's OIDC flow), replace an expired token.
Exec plugins run with the login-shell PATH and their stderr is shown. "Test all" tests every
context and puts ✓/✗ in the list.

**Exec consent and CA trust.** An exec plugin runs only after consent unless the file is a
loaded source and the plugin is identical to the saved one (`Kubeconfigs::needs_consent`,
keyed by a hash of the whole exec config, for the session, in memory). The dialog shows
command, args, env (secret-looking values masked), apiVersion, interactive mode and the
resolved path. "Test all" asks once for all plugins that need it. Phase 01's paste dialog now
lists a pasted kubeconfig's exec plugins and adds it only after "I checked these commands and
trust them" (`kubyl_kube::kubeconfig::exec_commands`, own commit). CA fetch (`tls::fetch_ca`):
an unverified handshake that sends nothing, then an anonymous `kube-public/cluster-info` read
over that connection; only certificates that verify the served chain are offered, with
subject, validity, SHA-256 and SPKI hash, and a confirmation checkbox. Setting a CA removes
`insecure-skip-tls-verify`.

**Creating and sharing** (`wizard`, `import`, `dialogs`). The wizard (name, cluster, credentials,
context, test, save) writes a Kubyl-owned 0600 file by default (or a path you pick) and can
connect afterwards. From a service account: create or pick one, TokenRequest with an expiry
(recommended in the UI) or a token Secret (warned), optional Role/ClusterRole binding
(cluster-admin warned, blocked on read-only clusters). Cloud CLIs: runs `aws eks
update-kubeconfig --kubeconfig <temp>`, `gcloud container clusters get-credentials` (with
`KUBECONFIG=<temp>`) or `az aks get-credentials --file <temp>` in a temp folder and opens the
result as a draft, so the entries are exactly what the CLIs write (instead of rebuilding them
from `describe`). Export a context with or without credentials (warning, 0600) or copy it as
YAML; copy/move contexts to another file, merge files, split a file per context.

**Integration.** After a save phase 01 reloads the file and rebuilds the edited contexts'
clients (live UI test). Renamed contexts keep their Kubyl overrides
(`ConnectionManager::move_context_settings`).

**Tests.** `cargo test -p kubyl_kubeconfig`: 46 unit and GPUI tests (writer, model, files,
certs, TLS against a local rustls server, validation, schema, conntest parts, editor with
real files: one-line save with backup, changed-on-disk banner and refused blind overwrite,
opt-in, rename and YAML sync; wizard). `kubyl_kube` got a test for the paste consent. Live,
ignored by default, all passing on kind (Kubernetes v1.37.0):

```sh
script/dev-cluster.sh
KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
  cargo test -p kubyl_kubeconfig --test live -- --ignored --nocapture --test-threads=1
KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig KUBYL_CREDENTIAL_STORE=memory \
  cargo test -p kubyl_kubeconfig --lib live_ui -- --ignored --nocapture --test-threads=1
# OIDC (script/oidc-dev.sh): add KUBYL_TEST_OIDC_KUBECONFIG=<the kubeconfig it prints>
# The real-expiry test waits 11 minutes: add KUBYL_TEST_SLOW=1
```

- Wizard for kind: CA fetched after comparing the fingerprint, client cert and key from
  files; the test passes (TLS, `kubernetes-admin`, list pods); after saving the context is a
  source and connects (live UI test, through the wizard's views).
- Opt-in editing of a commented copy of a kubeconfig under a throwaway HOME: the preview's
  diff is the one changed line, comments stay, a backup exists, `kubectl --kubeconfig` works.
- Wrong CA fails at TLS, a missing exec command at Credentials with the install hint, an
  expired token at Authentication (with a forged expired JWT and, slow test, a TokenRequest
  token that really expired), OIDC without a session at Credentials with "Sign in"; after
  signing in through Dex the test passes as `oidc:admin@kubyl.dev`.
- Service-account kubeconfigs (TokenRequest and token Secret) authenticate as the SA.
- A file changed on disk while editing shows the banner and the save is refused (GPUI test
  with real files).
- Secrets: an app run at `RUST_LOG=debug` (testing a context with an inline client key, then
  saving) was grepped for the key, PEM, token and bearer material: none. Nothing about the
  document goes to settings.json or state.json (tabs are restored by path; drafts live in
  memory only).

**Deferred / notes.**
- Keychain-backed credentials (`kubyl credential <id>`), decision 3's follow-up.
- Cloud CLI imports weren't run against real clouds (no accounts): the command lines are
  unit-tested against the CLIs' documented flags.
- Verified live on macOS only; Linux and Windows through CI.
- Two plan commits from other sessions landed on this branch (`37d61b2` phase 14 plan,
  `757852f` phase 15 plan); they only touch `plans/` and were left in.
- Environment: the disk filled up during the session (Docker's VM went read-only; Docker
  Desktop was restarted and kind came back on the same port). After a Docker restart
  `script/oidc-dev.sh` has to be re-run with `KUBYL_OIDC_DIR` set to the folder the cluster
  was created with (`docker inspect kubyl-oidc-control-plane` shows the CA mount), or Dex
  gets a new CA the API server doesn't trust.
- GPUI tests that close a gpui-component dialog need `gpui_component::Root` as the window's
  first view. `window.on_next_frame` doesn't fire while macOS doesn't drive frames (hidden
  window, screenshot runs): the YAML tab's scroll-to-entry retries on a timer instead.
