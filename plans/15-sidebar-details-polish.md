# Phase 15: Polish: connection dots, one entry per cluster and user, ConfigMap data

**Status:** not started
**Depends on:** 01 (kubeconfig loading, `ConnectionManager`), 02 (sidebar, details), 03 (`@` contexts in the palette). Part 3 should start after phase 11 has merged: phase 11 is changing `kubyl_kube` and the Clusters & kubeconfigs tab right now.
**Owns:** no crate of its own. Small, separate commits in shared crates: `kubyl_explorer` (sidebar, details), `kubyl_kube` (context grouping), `kubyl_palette` (`@` aliases), and settings lookups in `kubyl_metrics` (and `kubyl_alerts` once phase 14 exists)
**Mockups:** board 17 · Cluster status and ConfigMap data, to be added to `design/mockups/generate.py` first: the sidebar with status dots and grouped cluster rows (with the tooltip listing their contexts), and a ConfigMap's details with its data. Also update the shared `sidebar()` helper so every board shows the dots.

## Goal

Three small things that make daily use smoother:

1. See at a glance which clusters are connected.
2. See a ConfigMap's contents in its details and copy them.
3. Stop `oc` from filling the sidebar with one row per namespace ever visited: show one row
   per cluster and user.

The three parts are independent. Land them as separate commits, in any order (part 3 after
phase 11).

## 1. Connected clusters are visible at a glance

Today a cluster row (`kubyl_explorer::sidebar::clusters`, `Item::Root`) shows nothing when it is
connected, `…` while connecting, a yellow key when a sign-in is needed, and red "offline" or
"forbidden" text. Not connected and connected look the same until the row is expanded.

- [ ] One status slot at the right end of each cluster row (after the PROD badge): a green dot (`colors.green`, `kubyl_ui::StatusDot`) when connected, a pulsing dim dot while connecting, the yellow key when a sign-in is needed, a red dot when unreachable or forbidden, and nothing when not connected. The "offline"/"forbidden" text moves into the tooltip and the expanded status row. The icon keeps the cluster's color tag
- [ ] Tooltip on the cluster row: `Connected · 38 ms · v1.33.1`, or the error with the next retry
- [ ] Favorites: keep the cluster color dot; the cluster name is shown faint while that cluster isn't connected, and the tooltip gives the state
- [ ] The same green dot next to connected contexts in the palette's `@` list and the title bar cluster switcher, if they don't have one yet
- [ ] Optional: `explorer.cluster_order` = `name` (default) or `connected_first`, and a "Connected only" toggle in the Clusters section header (useful with 40 contexts)
- [ ] Screenshot test with connected, connecting, sign-in, unreachable and disconnected rows, in dark and light themes

## 2. ConfigMap data in the details

The details Summary has kind-specific sections (`kubyl_explorer::details`, `render_pod`,
`render_secret`…), but nothing for ConfigMaps. Secrets already get a "Data" section: one line
per key, masked, with reveal and copy buttons.

- [ ] "Data" section for ConfigMaps: one block per key, sorted by name. The header shows the key (monospace), its size and line count, a format chip (YAML, JSON, properties, shell, XML or text, from the key's extension and a content sniff) and a copy button (toast "Copied <key>")
- [ ] The block body shows the value as monospace text with whitespace kept. It shows the first 20 lines, with "Show all 240 lines" and collapse; soft wrap on by default with a toggle. Empty values show "(empty)". YAML gets highlighting if gpui-component's read-only code view makes that cheap; everything else stays plain
- [ ] `binaryData`: key, decoded size, a "binary" chip, copy as base64, "Save to file…"
- [ ] More than 8 keys: a key filter above the blocks, plus "Expand all" and "Collapse all"
- [ ] "Copy all" menu: as YAML (the `data` map), as `.env` (single-line values only; the menu says how many keys it skipped)
- [ ] An `immutable` chip when `immutable: true`
- [ ] Large ConfigMaps (up to 1 MiB): collapsed blocks only lay out their first lines, and an expanded block stops after 2,000 lines with "Open in YAML editor" for the rest. Arrowing through a ConfigMap list stays smooth
- [ ] "Used by", for ConfigMaps and Secrets: pods that reference the object through `volumes[].configMap`/`secret`, projected volumes, `env[].valueFrom.*KeyRef` and `envFrom`, grouped by their owner (Deployment, StatefulSet…) and per key where the reference names one. Built from the namespace's pods store; no extra API calls per row
- [ ] Secrets: revealed values use the same multi-line block instead of one truncated line. Masking, reveal and copy behave as before, and nothing is revealed by default

## 3. One entry per cluster and user (context grouping)

### What happens today

`oc login` writes one `cluster` entry per API server and one `user` entry per user and server
(`kube:admin/api-…:6443`). Every `oc project <ns>` (or `oc login -n`) then adds a context named
`<namespace>/<cluster>/<user>` and makes it the current context. Kubyl shows every context as its
own cluster row.

In the user's `~/.kube/config` (checked 2026-09-26, names and servers only): 37 contexts, 5 cluster
entries and 8 user entries. All 37 contexts are one of just 8 cluster + user pairs; they differ only
in their namespace. With grouping the sidebar shows 8 rows. `kube:admin` and the personal user on
the same server stay separate rows, as they must, because they have different permissions.

### When contexts form one entry

Contexts become one entry only when everything that affects the connection is identical:

- they come from the same kubeconfig file;
- they use the same `cluster` entry, or entries whose contents are identical (server, CA, TLS
  server name, proxy, insecure flag, extensions);
- they use the same `user` entry, or entries whose contents are identical (token, certificate
  and key, exec plugin with its args and env, auth provider, impersonation `as`/`as-groups`/`as-uid`).
  Credentials are only compared in memory, like today's reconnect `fingerprint`: never logged,
  stored, or written to disk as a hash;
- every other context field is equal except `namespace`.

Everything else stays separate: another user, other credentials, impersonation, different
proxy or TLS settings, and contexts in different files. Merging across files is out of scope for
this phase.

### Decisions (each with a recommendation; record them in the README)

1. **Ids.** A group's `ClusterId` must not change when `oc` adds contexts or switches the current
   context (every `oc project` does both). Otherwise tabs, connections and settings would jump.
   Recommended:
   - A group of two or more contexts gets an id built from the file and the cluster and user
     entry names, in a form no context id can take. Pick the exact format with care: context names
     can contain `/` and `@`.
   - A context that has no siblings keeps `<context>@<file>`, so kubeconfigs without duplicates
     see no change.
   - `ConnectionManager::resolve(id)` maps any member id to its group id. Everything that stored a
     member id goes through it: `state.json` (`kube.active`, open tabs), settings keys,
     `metrics.prometheus`, Argo CD's confirmed installs.
   - When a single context gets a sibling (the first `oc project` after `oc login`), its id changes
     to the group id. The live connection is re-keyed without reconnecting (same credentials), and
     open tabs and the active cluster follow. The same applies in reverse when the siblings go away.
2. **Settings.** Recommended:
   - A group reads its per-cluster settings (`kubernetes.contexts`) from the group id first, then
     from each member's id in a stable order: the file's current context first, then by name.
   - `production` or `read_only` on **any** member applies to the whole group; grouping must never
     drop a production marker.
   - `hidden` hides the group only when every member is hidden.
   - Color and display name come from the first member that sets them.
   - `namespaces` (offered when listing namespaces is forbidden) becomes the union of the setting
     and every member's namespace.
   - New changes are written under the group id. Member settings stay in the file, so they apply
     again if grouping is turned off.
   - `ConnectionManager::settings_keys(id)` returns the group id, the member ids and the member
     context names, so crates that key their own settings by cluster id or context name
     (`metrics.prometheus`, phase 14's `alerts.clusters`) keep finding them. Each of those crates
     gets a small follow-up commit.
3. **Label.** Recommended:
   - The `display_name` override wins.
   - For `oc`-style names (`<namespace>/<cluster entry>/<user>`), the label is the server host
     without a leading `api.` and without the default port, then ` · ` and the user name (the user
     entry name without its `/<cluster entry>` suffix). Examples:
     `oci.fra01.g-portal.se · alexander.birkner@g-portal.cloud`, `ocip.stl01.g-portal.se · kube:admin`,
     `0.0.0.0:55878 · system:admin`. This applies to `oc`-style contexts without siblings too, so
     all of them look alike.
   - Other groups: `<cluster entry> · <user entry>`.
   - The tooltip lists the member contexts and their namespaces. The label is used everywhere a
     cluster name appears (sidebar, favorites, title bar, tabs, palette).
4. **Namespace.** Recommended:
   - A group starts in its `default_namespace` setting. Otherwise it starts in the namespace of the
     file's current context when that context is a member, so after `oc project foo` in a terminal
     Kubyl opens that cluster in `foo` next time (it never switches while you're working). Otherwise
     in the last namespace used in Kubyl, and finally in the first member's namespace.
   - The members' namespaces are offered in the namespace picker, marked "from kubeconfig". This
     helps on OpenShift, where developers often can't list namespaces.

### Tasks
- [ ] `kubyl_kube`: compute groups while loading (`Loaded`), with ids, `resolve`, `settings_keys`, combined settings and the starting namespace as decided above. `contexts()` lists groups; `all_contexts()` still lists every context
- [ ] Recompute groups on every kubeconfig reload. Re-key connections when a group's id changes; never reconnect when only the namespace or the current context changed
- [ ] Sidebar, favorites, title bar and tabs show groups under their label, with the tooltip listing the members
- [ ] Palette `@`: lists groups, and member context names work as aliases, so `@dev-alex` still finds the right cluster and opens it in `dev-alex`
- [ ] Clusters & kubeconfigs tab (phase 01/11): shows the file as it really is (every context), with a group's members under an expander and the note "Shown as one cluster in the sidebar"
- [ ] Setting `kubernetes.group_contexts` (default `true`), and "Show Contexts Separately" in a cluster row's context menu (per group, kept in settings)
- [ ] Follow-up commits in `kubyl_metrics` (and `kubyl_alerts`) to look up per-cluster settings through `settings_keys`
- [ ] Unit tests:
  - a kubeconfig shaped like the user's (37 contexts become 8 groups; two users on one server stay separate);
  - entries with different names but identical content group;
  - near-misses stay separate: another token, impersonation, another proxy, another file;
  - the id stays stable while contexts are added and the current context changes;
  - re-keying doesn't reconnect;
  - a production flag on one member marks the whole group;
  - settings fall back to member ids;
  - the palette alias works;
  - no credential shows up in `Debug` output or logs
- [ ] `script/oc-contexts-dev.sh`: adds 30 `oc`-style contexts (one cluster and user entry, many namespaces) and a second user for the kind dev cluster, to a scratch kubeconfig

## Acceptance criteria

- Connected clusters show a green dot within a frame of connecting; the other states are
  distinguishable without expanding the row.
- A ConfigMap with multi-line YAML and JSON values, 30 keys, `binaryData` and close to 1 MiB of
  data: its details show every key, copying works with a toast, and scrolling and list navigation
  stay smooth. Secrets show multi-line values only after reveal.
- The user's `~/.kube/config` shows 8 cluster rows instead of 37, with readable labels. The
  personal user and `kube:admin` on the same server are separate rows. Colors and PROD markers set
  on the old contexts still apply.
- Running `oc project x` in a terminal while Kubyl runs adds no row, doesn't reconnect, and keeps
  open tabs.
- Turning `kubernetes.group_contexts` off brings back one row per context with their old settings.

## Risks

- Grouping two different identities would be a safety bug: the user would act as someone other
  than who they see. Only exact matches group, tests cover the near-misses, and the tooltip always
  shows the user.
- The id change from a single context to a group touches persisted state across crates.
  Everything that reads persisted cluster ids must go through `resolve`.
- Phase 11 is editing `kubyl_kube` and the Clusters & kubeconfigs tab at the moment. Parts 1 and
  2 can land any time; part 3 should wait for phase 11's merge (or rebase onto it).

## Later (not in this phase)

- "Remove redundant contexts" in the kubeconfig editor (phase 11), with its diff preview and backup
  (`oc` recreates them on the next `oc project`, so grouping is the real fix).
- Grouping identical contexts across files.
- "Restart workloads using this ConfigMap" from the "Used by" list.

## Handoff log
