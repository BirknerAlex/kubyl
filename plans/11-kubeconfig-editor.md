# Phase 11: Kubeconfig editor

**Status:** not started
**Depends on:** 01 (sources, auth, client building, file watching), 04 (YAML editor, schema validation and diff for the raw view and the save preview)
**Owns:** `crates/kubyl_kubeconfig` (new)
**Mockups:** none yet. Add board 11 · Kubeconfig editor (editor tab, "Test connection" results, "New kubeconfig" wizard) to `design/mockups/generate.py` before building the UI.

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
- [ ] Kubeconfig document model: clusters, users, contexts, `current-context`, preferences and extensions. Unknown fields survive a round trip
- [ ] Load and save per file: atomic write (temp file + rename), mode 0600 for new files and for files with inline credentials (otherwise keep the existing mode), timestamped backups (`<config dir>/kubeconfig-backups/`, keep the last N), refuse to overwrite a file whose hash changed since it was loaded
- [ ] Changed on disk while editing (phase 01's watcher): a banner with "Reload" and "Keep mine" (shows the diff)
- [ ] Validation as you type: contexts pointing at missing clusters or users, duplicate names, invalid server URLs, unreadable certificate or key files, client certificates and CAs that are expired or expire soon (parse the PEM, show NotAfter), `insecure-skip-tls-verify` together with CA data
- [ ] `$KUBECONFIG` merges several files: the editor edits one file at a time, and the merged view offers "open the file that defines this"

### Editor
- [ ] Opens from the Clusters & kubeconfigs tab (source row "Edit…", context row "Edit context…"), the Explorer "+" menu ("New kubeconfig…") and the palette
- [ ] Layout: a list on the left grouped into Contexts, Clusters and Users (add, duplicate, rename, delete; deleting a cluster or user that contexts still use asks first), a form on the right, and a "YAML" tab with the raw file (phase 04 editor with the kubeconfig schema), kept in sync with the form
- [ ] Cluster form: server URL, CA (system trust store, file, pasted PEM, or fetched from the server with the fingerprint shown for confirmation), TLS server name, proxy URL, insecure skip verify (with a red warning), disable compression
- [ ] User form by auth type: token (masked, reveal on click), token file, client certificate and key (file or inline PEM; shows subject, issuer, expiry), exec plugin (command, args, env, apiVersion, interactive mode, provide cluster info, install hint) with presets for AWS EKS, GKE (`gke-gcloud-auth-plugin`), AKS (`kubelogin`) and kubelogin OIDC, OIDC auth provider (issuer, client ID and secret, scopes, extra parameters), basic auth (marked deprecated)
- [ ] Context form: cluster and user pickers, namespace (a live list once a test succeeded). Kubyl's own overrides (display name, color, production, read-only) stay in settings.json and are shown next to it
- [ ] Secrets are masked by default, never logged, never written to `state.json`; copying one is an explicit action

### Verify and test
- [ ] "Test connection" for a context, from the in-memory edits, without saving (phase 01's client builder). Before it runs an exec plugin that comes from an import or unsaved edits, it shows the command and arguments and asks for consent (`interactiveMode` isn't consent). Steps, each with ✓/✗ and a plain-language error: DNS and TCP, TLS (handshake, CA verification, server certificate details), `/version`, authentication (`SelfSubjectReview`: user name and groups), permissions (can list namespaces, can list pods, cluster-admin or not), latency
- [ ] Failures suggest the fix: "certificate signed by unknown authority: fetch the CA from the server?", "token expired", "`aws` isn't installed: see the install hint"
- [ ] After consent, exec plugins run with the login-shell `PATH` (phase 01) and show their stderr. OIDC runs the sign-in flow
- [ ] "Test all contexts" of a file, results shown in the list

### Creating and sharing
- [ ] "New kubeconfig…" wizard: name, cluster (server, CA), credentials, context, test, save (Kubyl-owned by default, or pick a path)
- [ ] From a service account on a connected cluster: create or pick a ServiceAccount, get a token (TokenRequest with an expiry, or a token Secret), optionally bind a Role or ClusterRole, generate the kubeconfig
- [ ] From cloud CLIs when they're installed: EKS (`aws eks describe-cluster`), GKE (`gcloud container clusters describe`), AKS (`az aks show`), producing the same entries their `get-credentials` commands write
- [ ] Export one context as a standalone kubeconfig, with or without credentials (warns when they're included); copy as YAML
- [ ] Move or copy contexts between files, merge files, split a file into one file per context

### Integration
- [ ] After saving, phase 01 reloads the file; edited contexts reconnect and their clients are rebuilt
- [ ] Palette: "Kubeconfig: New…", "Kubeconfig: Edit Current Context", "Kubeconfig: Test Current Context"
- [ ] Settings: number of backups kept, whether external files may be edited at all

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
