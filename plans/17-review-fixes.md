# Phase 17: Code-review fixes

**Status:** done (all 96 findings have a commit; see the handoff log for the partial ones)

Findings from a full code review (all 25 crates) for bugs, security issues and potential crashes.
Each finding was checked against the source at commit `af6b722` (v0.3.3); line numbers may drift.
F01, F02 and F03 were confirmed by hand, the rest by the reviewing agents. Nothing was compiled or
run during the review, so **reproduce first, then fix** (add a regression test where practical).

## How to work through this file

- Work **one work package (WP) per session**. A WP defines a bounded scope and may span multiple
  crates, so the session reads only the files it needs.
- Tick a finding's checkbox only when the fix is merged on the branch and tests pass. If a finding
  turns out to be invalid, tick it and write `INVALID: <reason>` after it.
- Add a dated line to the **Handoff log** at the bottom after every WP.
- Per `CLAUDE.md`: no network or blocking calls on the UI thread; never log or persist tokens or
  Secret data. Shared crates (`kubyl_ui`, `kubyl_kube`, `kubyl_core`) go in a small separate PR.
- Severity: **H** high, **M** medium, **L** low.

Suggested order: WP1 (H), then WP2 to WP5 (security), then the rest.

---

## WP1: Highest priority (do first)

- [x] **F01 (H)** Download can delete the parent of the download folder. `kubyl_files/src/transfer.rs:385-389`, `listing.rs:185-283`. Names from the container are never checked for `/` or `..`; a newline in a file name fakes an `ls` entry `x/..`, so `dest_name` becomes `..` and `remove_dir_all(job.local.join(".."))` runs. Fix: reject names containing `/`, and `.`/`..`, in the listing parsers and again at job creation; never `remove_dir_all` a path that is not a direct child of `job.local`.
- [x] **F02 (H)** Read-only, RBAC and PROD typed-confirmation checks only look at the first item of a selection. `kubyl_explorer/src/actions.rs:370-395` (`targets`), `:497` (`typed_confirmation`), `:481-525` (`run_each`). In Favorites, select-all mixes clusters, so a mutation can hit a read-only prod cluster. Fix: check every distinct cluster in the selection (read-only, RBAC, prod), name the cluster in the dialog lines (`line()` at `:365`), and apply to Scale, Restart, Pause, Suspend, Cordon, Trigger.
- [x] **F03 (H)** Panic while typing in the Network Flows filter bar. `kubyl_netflow/src/filter.rs:979-982`. `rfind(char::is_whitespace) + 1` assumes a 1-byte space; U+00A0 (Option+Space on macOS) or U+3000 gives a non-char-boundary slice. Fix: use the matched char's `len_utf8()`. Add a test with a non-breaking space.

## WP2: `kubyl_files` and `kubyl_terminal` (path escape, pods, paste)

- [x] **F04 (M)** Unchecked entry names escape the local folder in drag-out staging (`kubyl_files/src/view.rs:1342-1348`) and image preview (`view.rs:1726`); they use the raw name instead of `file_name()`. A tab in a symlink target (`find -printf`) or the `ls` newline trick can inject a name like `../../x`. Fix together with F01 by sanitising at parse time.
- [x] **F05 (M)** Folder download hangs forever when local extraction fails. `kubyl_files/src/transfer.rs:343-365`. Stdout is no longer drained, the remote `tar` blocks, the job stays Running and holds a pod slot. Also `tx.send` (std `SyncSender`) blocks a Tokio worker. Fix: keep draining stdout on error (or kill the exec), use an async channel.
- [x] **F06 (M)** Privileged node-shell pods outlive the app. `kubyl_terminal/src/view.rs:179-184`, `:293-302`, `:614-624`, `exec.rs:400-407`. No `on_app_quit` cleanup; a tab closed during `create` can leave a pod without a guard; nothing sweeps by `NODE_SHELL_LABEL`. Fix: delete on quit (awaited), delete on failed/disconnected tab, sweep leftovers by label on connect. **Done, partly:** pod deleted on quit/failure/aborted create, finished orphans swept; Running orphans are not swept (may belong to another window), the 12 h deadline still covers a crash.
- [x] **F07 (M)** Bracketed-paste filter can be bypassed. `kubyl_terminal/src/input.rs:219`. A single `replace("\x1b[201~", "")` lets `"\x1b[20\x1b[201~1~"` rebuild the end marker. Fix: strip all ESC, or loop until no change.
- [x] **F08 (M)** Terminal output batching has no cap; it drains the whole channel into one `Vec` and parses on the UI thread. `kubyl_terminal/src/view.rs:627-631`. Fix: cap the batch (logs view uses 2000).
- [x] **F09 (L)** Container files are overwritten non-atomically (`cat > "$1"`), stdin write errors ignored. `kubyl_files/src/remote.rs:333-343`, `transfer.rs:531`. Fix: write to a temp name then `mv`, surface stdin errors.
- [x] **F10 (L)** A stale `.name.kubyl-part` is resumed into a changed file. `kubyl_files/src/transfer.rs:257-277`. Fix: key the part file by size/mtime (or checksum) of the remote file.
- [x] **F11 (L)** `tempfile::tempdir().expect("temp dir")` on the UI thread panics when temp is full. `kubyl_files/src/view.rs:1697`. Fix: handle the error with a toast.
- [x] **F12 (L)** Blocking local filesystem calls on the UI thread. `kubyl_files/src/view.rs:1006`, `1212`, `1238`, `1532`, `1577`. Fix: move to the background executor.

## WP3: `kubyl_updates` (cloud providers, upgrade safety)

- [x] **F13 (M)** Windows command injection through `cmd /C` for `.cmd`/`.bat` shims. `kubyl_updates/src/providers/cloud.rs:251-266`; values reach it from `aks.rs:177-191` (tenant) and `eks/credentials.rs:121-147` (profile, role-arn, region). Fix: validate those values against a strict charset, and/or resolve the shim to the real executable, or escape for cmd.exe.
- [x] **F14 (M)** EKS endpoint host built from an unchecked region, so signed requests (with the session token) can go to another host. `providers/eks.rs:164-167`, `:95-110`. Fix: validate the region (`^[a-z]{2}(-[a-z]+)+-\d$`).
- [x] **F15 (M)** SUC (k3s) writes accept any version and skip safety checks; offline fallback patches `spec.version: "1.34"`. `suc.rs:367-383`, `view/mod.rs:199-208`, `view/cards.rs:1394-1437`. Fix: require the version to come from the fetched channel list, block downgrade and writes while a Plan is applying, and disable the action when channels can't be fetched.
- [x] **F16 (M)** Confirm dialog can skip the failed-check acknowledgement while pre-flight is still running. `service.rs:436-452`, `view/mod.rs:316-321`, `view/confirm.rs:118-147`. Fix: disable Update until checks finish, and re-evaluate the acknowledgement when results arrive.
- [x] **F17 (L)** A rekey during a write leaves the cluster stuck busy. `service.rs:209-217`, `:586-597`. Fix: clear `busy` through the current id after a `Rekeyed` event.
- [x] **F18 (L)** EKS update errors are dropped and history capped at 30. `providers/eks.rs:1238-1262`. Fix: keep errors, describe running updates first.
- [x] **F19 (L)** Integer overflow on server-supplied versions. `version.rs:52-60`, `126-134`, `eks.rs:309`, `gke.rs`/`aks.rs` `next`. Fix: `checked_`/`saturating_` arithmetic.
- [x] **F20 (L)** KubeadmControlPlane list errors are swallowed, which also skips the workers-vs-control-plane guard. `capi.rs:74-83`, `:329-340`.
- [x] **F21 (L)** OpenShift merge patch can pair the new version with a stale image. `openshift.rs:479-482`. Fix: set `image: null` when the target has none.
- [x] **F22 (L)** Short-lived credentials are never cached (`fresh_until` already in the past), so the CLI spawns on every read. `providers/cloud.rs:448-453`.

## WP4: `kubyl_explorer` and `kubyl_kube` (multi-cluster safety, auth)

- [x] **F23 (M)** Scale from the list or palette has no typed confirmation on PROD, even to 0. `kubyl_explorer/src/actions.rs:534-570`. Set `spec.typed`/`spec.danger` like the details path (`details.rs:2218`).
- [x] **F24 (M)** Copy YAML puts full Secret data on the clipboard. `kubyl_explorer/src/actions.rs:833-904`. Fix: mask Secret data (or ask for reveal) in `copyable_yaml`.
- [x] **F25 (M)** List row identity is `(source index, key)`, so selection jumps to another cluster's object when sources are rebuilt. `list/rows.rs:10`, `:163`, `list/mod.rs:567-610`, `:768-772`. Fix: include the cluster id in `RowId`, and republish the selection when sources change.
- [x] **F26 (M)** OAuth loopback callback stalls on an idle connection (up to 600 s) and `?error=` aborts without checking `state`. `kubyl_kube/src/auth/oidc.rs:646-673`. Fix: per-connection read timeout, handle connections concurrently, validate `state` first.
- [x] **F27 (M)** OIDC refresh failure of any kind discards the refresh token. `oidc.rs:310-315`. Fix: only drop it on `invalid_grant`.
- [x] **F28 (L)** Pending or confirmed scale is dropped silently when the details target changes. `details.rs:263-264`, `:2264-2300`.
- [x] **F29 (L)** Partial failures reported as full success. `actions.rs:489-497`. Report skipped items.
- [x] **F30 (L)** Overlapping kubeconfig reloads apply stale results. `kubyl_kube/src/manager.rs:442-460`. Add a generation check.
- [x] **F31 (L)** Blocking filesystem calls on the UI thread. `manager.rs:884-915` (`update_watcher`).
- [x] **F32 (L)** RBAC checks never repeated after reconnect or after a failed check. `sidebar/clusters.rs:352`, `list/mod.rs:551`. Clear `rbac_requested` on connect.
- [x] **F33 (L)** `disconnect` resets `caps` to default, so PROD/read-only is wrong while disconnected. `manager.rs:1110-1120`.
- [x] **F34 (L)** New OpenShift token accepted unless the check returns exactly 401. `auth/openshift.rs:299-303`.
- [x] **F35 (L)** Cordon/Uncordon run with no confirmation, even on PROD. `actions.rs:707-725`.
- [x] **F36 (L)** Dev file credential store loses entries on concurrent writes (`KUBYL_CREDENTIAL_STORE=file`). `auth/store.rs:84-114`. Add a lock.

## WP5: `kubyl_alerts` and `kubyl_operators`

- [x] **F37 (M)** Silence/acknowledge can hit the wrong cluster; selection stored by fingerprint only. `kubyl_alerts/src/view/mod.rs:448-477`, `details.rs:170`. Key the selection by (cluster, fingerprint).
- [x] **F38 (M)** Copied `helm uninstall`/`rollback` commands lack `--kubeconfig`. `kubyl_operators/src/helm/present.rs:331-338`, `view/helm.rs:51`. Pass `ContextInfo.file`.
- [x] **F39 (M)** Uninstall with "delete CRDs" wipes other installs' resources. `dialogs.rs:1579-1605`, `olm/ops.rs:369-393`, `434-445`. Check whether another CSV owns the CRDs; warn or skip.
- [x] **F40 (M)** Alerts freeze after a cluster rekey (`discovering`/`in_flight` stuck). `kubyl_alerts/src/service.rs:531-536`, `:745-760`, `:840-846`.
- [x] **F41 (M)** `drop_cluster` does not bump the generation, so stale results apply after reconnect and forwards leak. `service.rs:504-512`, `:587`. Stop the previous `state.forwards` in `discovered()`.
- [x] **F42 (M)** Alertmanager outage causes a notification storm and shows silenced alerts as firing. `service.rs:1547-1566`, `:870-900`, `merge.rs:61`. Keep the previous state when every Alertmanager read fails; don't reset `failures`.
- [x] **F43 (L)** Dialog lists CRDs that are then silently kept. `kubyl_operators/src/dialogs.rs:1583-1600` vs `:1679-1686`.
- [x] **F44 (L)** OperatorHub stuck on Loading after rekey. `kubyl_operators/src/service.rs:259-266`, `:444-470`.
- [x] **F45 (L)** Older alerts fetch can overwrite a newer one (`refetch_at` ignores `in_flight`). `service.rs:626-630`.
- [x] **F46 (L)** Failed silences read leaves "no silences" with no error. `service.rs:1518-1520`.
- [x] **F47 (L)** Target check says "deleted" for namespaced targets without a namespace label; label values unencoded in path. `view/details.rs:121-131`.
- [x] **F48 (L)** PrometheusRule lookup cached forever (`Ready`/`Failed`). `view/rules_tab.rs:92-107`.
- [x] **F49 (L)** Undo failures are lost; `recreate_now` skips the read-only check. `silence.rs:517-532`.
- [x] **F50 (L)** Duplicate uninstall possible via Enter while busy. `dialogs.rs:1503-1506`, `:1568-1575`.
- [x] **F51 (L)** CSV icons decoded on every frame. `kubyl_operators/src/widgets.rs:282-297`. Cache like the hub icons.

## WP6: `kubyl_argocd` and `kubyl_charts`

- [x] **F52 (M)** Panic on huge sync-window duration; huge valid durations freeze the UI. `kubyl_argocd/src/windows.rs:176`, `:179-199`, `columns.rs:197`. Use `try_minutes`/`checked_sub`, cap the duration and loop count.
- [x] **F53 (M)** One-click Terminate and auto-sync/prune/self-heal toggles, no confirmation even on PROD. `views/app.rs:895-925`, `:1170-1190`, `dock.rs:530-545`, `actions.rs:185-192`. Add confirmation, typed on production (like Rollback at `dialogs.rs:808`).
- [x] **F54 (M)** Sign-out doesn't cancel a sign-in in progress; token still stored and install trusted. `state.rs:679-717` vs `565-665`. Cancel `_sign_in`, check the session before `store_session`/`trust`.
- [x] **F55 (M)** Leaked port-forwards; `forward_stopped` marks the wrong session failed. `state.rs:902-930`, `397-406`, `571-577`, `743-751`. Record the ForwardId immediately, stop it on drop, match ids in `forward_stopped`.
- [x] **F56 (L)** `glob_match` is exponential on patterns like `*a*a*a*b`, on the UI thread. `windows.rs:206-216`. Use an iterative matcher.
- [x] **F57 (L)** "Open Argo CD UI" opens argocd-cm `url` unchecked. `actions.rs:354-363`. Allow only http(s).
- [x] **F58 (L)** Old API results shown after disconnect or new sign-in. `views/app.rs:205-216`, `437-496`. Drop `_api_task`/`_diff_task` on disconnect.
- [x] **F59 (L)** Kubernetes-mode delete can patch finalizers and then fail to delete. `ops.rs:527-556`, `run.rs:249`. Pre-check `patch` too, add a UID precondition.
- [x] **F60 (L)** Latent slice panic for Area/StackedArea when a series is longer than `times`. `kubyl_charts/src/paint.rs:67`.
- [x] **F61 (L)** `short_repo` can show part of a password (splits at first `@`). `kubyl_argocd/src/model.rs:268`. Split at the last `@` before the path.

## WP7: `kubyl_settings`, `kubyl_kubeconfig`, `kubyl_selfupdate`

- [x] **F62 (M)** A broken settings.json is overwritten with `{}` plus one section. `kubyl_settings/src/store.rs:276-286`, `:108-126`; also `reload_from_str`. Fix: never write when the file failed to parse; back it up first.
- [x] **F63 (M)** Panic when a section is not a JSON object (`null`, `[]`). `store.rs:210-217`. Replace the `expect` with a reset-to-object.
- [x] **F64 (M)** Concurrent settings writes share `settings.json.tmp`. `paths.rs:481-487`, `store.rs:119-125`. Use a unique temp name and serialise writes.
- [x] **F65 (M)** Blocking file read on every render in the wizard. `kubyl_kubeconfig/src/wizard.rs:1273-1279`. Cache and read on the background executor.
- [x] **F66 (L)** Exec-plugin consent ignores the folder a relative command resolves against. `model.rs:970-976`, `state.rs:223-236`. Include the resolved path in the consent key.
- [x] **F67 (L)** Service-account import can return a token for another identity; bindings checked by name only. `import.rs:205-239`, `147-160`, `176-186`. Verify Secret type and `kubernetes.io/service-account.name`, and binding subjects.
- [x] **F68 (L)** Preview update channel always 404s; manifest channel not checked. `kubyl_selfupdate/src/download.rs:12`, `service.rs:133-145`, `.github/workflows/release.yml:452`.
- [x] **F69 (L)** `.expect("apply task panicked")` re-raises and crashes the app. `service.rs:163-165`. Map the join error to Failed.
- [x] **F70 (L)** macOS bundle check doesn't pin the signing identity. `apply.rs:165-176`. Add a `-R` requirement with the Team ID.
- [x] **F71 (L)** State and settings changes can be lost at quit. `kubyl_settings/src/state.rs:601-640`, `store.rs:119`. Await in-flight writes on quit.
- [x] **F72 (L)** Config dir falls back to shared `/tmp/kubyl` when HOME is unset. `paths.rs:471-473`. Use a private per-user dir (0700) or fail.
- [x] **F73 (L)** Finished tasks pile up in `_tasks` on every reload. `kubyl_kubeconfig/src/editor.rs:239`.

## WP8: `kubyl_netflow` and `kubyl_webview`

- [x] **F74 (M)** Loki poll can loop forever without sleeping when 5000 or more records share one timestamp. `kubyl_netflow/src/backends/netobserv.rs:205-229`. Advance past the boundary or sleep when the cursor doesn't move.
- [x] **F75 (M)** Cluster-chosen Loki URL (`spec.loki.*.url` with an IP or localhost) is requested from the desktop. `detect.rs:537-541`, `:657`, `service.rs` (`LokiTarget::Url`). Restrict to Service targets or ask for confirmation.
- [x] **F76 (M)** Idle web view tab keeps JS running after its port is freed; cookies are host-scoped, so another forward on the same port gets the Argo token. `kubyl_webview/src/view.rs:745-760`, `session.rs:70`. Destroy or navigate the native page to `about:blank` when the forward stops.
- [x] **F77 (M)** Switching to Private is ignored while the view is being created. `view.rs:884-899`. Re-create the view if `private` changed while `creating`.
- [x] **F78 (L)** Accepted certificates saved in Private mode. `view.rs:915-919`. Guard with `!self.private`.
- [x] **F79 (L)** No navigation policy; `window.open` goes to the system browser unprompted. `view.rs:595-608`, `native/mod.rs:262`.
- [x] **F80 (L)** `regex_escape` doesn't escape `"` and triples backslashes. `netobserv.rs:373-383`.
- [x] **F81 (L)** Unchecked integer arithmetic on cluster data. `netobserv.rs:569`, `731`, `aggregate.rs:254-259`, `390-405`.
- [x] **F82 (L)** Relay forward leaked on `Rekeyed` when `to` already exists. `service.rs:901-906`.
- [x] **F83 (L)** `Seen` completion counts drift once a field passes 2000 values. `buffer.rs:152-176`.
- [x] **F84 (L)** Header mask misses `x-vault-token`, `private-token`, `x-csrf-token`, `x-goog-api-key`. `sanitize.rs` (`SECRET_HEADERS`).

## WP9: `kubyl_portforward`, `kubyl_resources`, `kubyl_palette`, `kubyl_logs`, `kubyl_yaml`

- [x] **F85 (M)** Port-forwards are never stopped when their cluster disconnects, reconnects or is removed. `kubyl_portforward/src/manager.rs:134`, `:211`, `lib.rs:175`. Subscribe to `ConnectionEvent` and stop or restart forwards.
- [x] **F86 (M)** Applying a Secret can restore stale data after a server-side rotation (masked values compare equal). `kubyl_yaml/src/view.rs:602-605`, `keep_mine` at `689-696`. Compare real values, not the masked render.
- [x] **F87 (M)** Log lines have no length limit; whole logs loaded into memory. `kubyl_logs/src/stream.rs:738`, `fetch()`/`fetch_all()`; `detect_level` parses JSON on the UI thread (`view.rs:953`). Cap line and total bytes.
- [x] **F88 (M)** Pod CPU/memory % wrong when only some containers set limits. `kubyl_resources/src/metrics.rs:107-113`, `:128-157`. Treat the pod as unlimited if any container lacks a limit.
- [x] **F89 (L)** YAML alias expansion ("billion laughs") on the UI thread. `kubyl_yaml/src/parse.rs:241-242`. Cap expansion, parse off the UI thread. **Done, partly:** alias expansion is capped at 100k nodes; parsing was not moved off the UI thread (bounded now).
- [x] **F90 (L)** `parse_quantity` can't parse `E`/`Ei`. `kubyl_resources/src/format.rs:127-133`.
- [x] **F91 (L)** `LIMIT - ix` can underflow with more than 50 stored entries. `kubyl_palette/src/recent.rs:44`. Trim on load, use `saturating_sub`.
- [x] **F92 (L)** IPv6 bind address not bracketed in URLs; `active()` always reports `localhost`. `kubyl_portforward/src/manager.rs:140-151`, `:381`.
- [x] **F93 (L)** Synchronous `TcpListener::bind` on the UI thread and a TOCTOU race. `kubyl_portforward/src/lib.rs:259`.
- [x] **F94 (L)** Auto-start and saved forwards skip the `read_only` check. `lib.rs:337`, `:376`.
- [x] **F95 (L)** Workload forward captures the client before resolving the selector, and starts after disconnect. `lib.rs:420-432`.
- [x] **F96 (L)** Logs view `self.pods` chips grow forever with pod churn. `kubyl_logs/src/view.rs:854-860`.

---

## Handoff log

_Append dated notes here after each WP: what was fixed, what was deferred, what the next session must know._

- 2026-09-28: Plan created from the review. No fixes applied yet.
- 2026-09-28: All 96 findings have a commit, one each (`fix(Fxx): ...`) with a regression test where one was practical (GPUI-window and live-cluster paths are untested; the commit bodies say so). No finding was judged invalid or deferred. Partial: F06 (Running orphan node-shell pods are not swept) and F89 (parse stays on the UI thread, but is capped). Two follow-up commits fixed Windows CI for F09 (dead test helper) and F66 (rooted commands count as absolute).
- Notes for reviewers: F87 caps non-follow log fetches at 64 MiB, which returns the oldest bytes for a larger log. F02 checks read-only/PROD for the whole selection, which is stricter than Drain (first node only) needs. F18 removes the 30-update describe cap instead of describing running updates first. Whole-workspace `cargo test`/`cargo deny` were not run locally; CI is the check.
