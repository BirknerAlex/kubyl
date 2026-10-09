# Phase 21: Agents over ACP (Claude, Codex, Gemini, Copilot) with cluster tools

**Status:** in progress: part 1 (read tools, ACP client, panel, agent questions) done on branch `phase/21-agents-acp`; part 2 (writes) not started
**Depends on:** 02 (`ResourceStores`, actions, details), 04 (diff and apply), 05 (terminal renderer, logs), 07 (events, `MetricsService`); 14 and 18 optional (alerts and PromQL tools, "Ask agent" on an alert)
**Owns:** `crates/kubyl_agent` (new), `crates/kubyl_agent_core` (new), the `kubyl mcp-bridge` subcommand in `crates/kubyl`
**Mockups:** board 19 · Agents in `design/mockups/generate.py`: `Agent` (a thread with Kubyl tool calls, a plan, a command to approve with its warning, the "Ask agent" chip, the status bar item), `AgentStates` (new thread: first-run note, cluster, agent picker with install commands; sign-in and stopped states) and `AgentQuestions` (an agent's form, a link to open, a file change as a diff). The proposed apply (part 2) has no board yet.

## Goal

Ask the user's own coding agent (Claude, Codex, Gemini CLI, Copilot CLI, or any other ACP agent)
about what Kubyl shows: "why does this pod crash", "what changed before these alerts", "write a
NetworkPolicy for this namespace". Kubyl is the **client**, like Zed: it starts the agent, shows
the conversation in a panel, and gives the agent Kubyl's cluster tools. Kubyl holds no API keys and
pays for nothing; the agent signs in on its own.

Two things make this better than pasting into a CLI:

- **Kubyl decides what the agent sees.** The agent reads the cluster through Kubyl's tools, which
  apply Kubyl's masking (Secret data, tokens) and the user's RBAC. It never gets a kube token.
- **Kubyl decides what the agent does.** Every write is a proposal the user approves in Kubyl's
  diff view, with the PROD confirmation and read-only clusters working as everywhere else.

## ACP in brief (what this phase builds on)

The [Agent Client Protocol](https://agentclientprotocol.com) is JSON-RPC 2.0 over the agent's
stdio. The client starts the agent as a child process.

- Client to agent: `initialize` (versions and capabilities), `authenticate`, `session/new` (`cwd`,
  `mcpServers`), `session/prompt`, `session/cancel` (notification), and optionally `session/load`,
  `session/resume`, `session/set_mode`, `logout`.
- Agent to client: `session/update` notifications (message chunks, thoughts, tool calls and their
  updates, plans, available commands, mode changes), `session/request_permission`, and, when the
  client advertises them, `fs/read_text_file`, `fs/write_text_file`, `terminal/create`,
  `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, `terminal/release`, and
  `elicitation/create`.
- MCP servers are handed to the agent in `session/new`. Stdio servers (`name`, `command`, `args`,
  `env`) must be supported by every agent; HTTP servers (`type: "http"`, `url`, `headers`) only
  when the agent's `mcpCapabilities.http` is true.
- Paths are absolute, lines 1-based. Extensions go in `_meta` or `_`-prefixed methods.
- Agents: Gemini CLI speaks ACP itself; Claude goes through Zed's `claude-agent-acp` adapter,
  Codex through `codex-acp`; Copilot CLI has ACP in public preview. The ACP registry
  (`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`) lists 50+ agents with
  `npx`, `uvx` or binary distributions.

**Licenses.** The `agent-client-protocol` crate is Apache-2.0 and is what Zed uses. Zed's agent
crates (`agent_ui`, `acp_thread`, …) are GPL: we reuse the protocol, never their code. `cargo deny
check` decides for every new crate.

## Decisions to make first

Each has a recommendation. Record the outcomes in the README's decision table.

1. **Crates.** Recommended: `kubyl_agent_core` (no GPUI: the ACP client, agent discovery and
   processes, the MCP server and its tools, masking, the permission policy, the thread model) and
   `kubyl_agent` (the panel, actions, prompts). The core is a service on `Host` from the start
   (phase 20's pattern), so its tests use `TestHost`.
2. **Protocol and MCP crates.** Decided: `agent-client-protocol-schema` (the wire types only,
   Apache-2.0, pinned `=1.10.2`, default features off) and Kubyl's own small JSON-RPC loop
   (`kubyl_agent_core::jsonrpc`). The `agent-client-protocol` 3.x SDK is a builder framework
   with its own process and stdio adapters that would sit beside Kubyl's Tokio runtime and
   `Host` services. The MCP server is Kubyl's own too (`initialize`, `ping`, `tools/list`,
   `tools/call` over streamable HTTP with JSON answers, about 300 lines on the `hyper` already in
   the tree) instead of `rmcp`, whose server pulls in its own HTTP stack for the same four
   methods. `cargo deny check` is green.
3. **Which agents.** Recommended: a built-in list of known agents with their launch command and
   install hint (Claude via `claude-agent-acp`, Codex via `codex-acp`, Gemini CLI, Copilot CLI;
   verify each command and flag when implementing, they move fast), found on `PATH` and the usual
   install dirs (npm global, Homebrew, `~/.local/bin`), plus custom agents from settings
   (`command`, `args`, `env`). **Kubyl never installs or downloads an agent** in this phase: a
   missing one shows its install command to copy. The registry and one-click installs are "Later".
4. **One cluster per thread.** Recommended: a thread is bound to one cluster (and user) when it
   starts, shown with the cluster's color dot like a tab. The tools only reach that cluster.
   Switching clusters starts a new thread. This keeps "which cluster did it touch" obvious and
   matches the PROD and read-only flags.
5. **How the agent reaches Kubyl's tools.** Recommended: one MCP server per thread, served
   in-process on Tokio on `127.0.0.1` with a random per-thread bearer token (memory only). Agents
   with `mcpCapabilities.http` get it as an HTTP server with the token in `headers`. The others
   get a stdio server: `kubyl mcp-bridge` (the Kubyl binary itself, `current_exe()`), which reads
   the URL and token from its environment and relays stdio to the same endpoint. One
   implementation, two transports.
6. **The agent's own access to Kubernetes.** The agent runs with the user's rights on the user's
   machine, so Kubyl can't sandbox it; it can only make the safe path the easy one.
   Recommended:
   - Start the agent with `KUBECONFIG` pointing at an empty, Kubyl-owned file, so `kubectl` in
     the agent's shell reaches nothing by default and the agent uses Kubyl's tools.
     `agent.kubectl: "none" | "context"` in settings: `context` writes a one-context kubeconfig
     for the thread's cluster, **only when its user has no inline credentials** (exec plugins,
     OIDC via kubelogin); otherwise the setting says why it can't.
   - Advertise the `terminal` capability, so agents that support it run their shell commands
     through Kubyl, where each command is shown and needs approval (policy below).
   - Say it plainly in the panel's first-run note and the docs: an agent can still read files
     like `~/.kube/config` with its own tools; Kubyl's masking covers what goes through Kubyl.
7. **Writes.** Recommended: read-only tools in part 1. Part 2 adds write tools that only
   **propose**: the agent sends a manifest, patch or action; Kubyl shows it in phase 04's diff
   view (or a summary for scale, restart, delete), the user applies or rejects, and the tool
   returns the outcome. Hidden on read-only clusters; PROD confirms like every other write. No
   write ever runs without the user's click.
8. **Working directory.** Recommended: a per-thread scratch dir
   (`<cache dir>/kubyl/agent/<thread id>`, removed with the thread), or a folder the user picks
   (a GitOps repo, so the agent can edit manifests there). Kubyl advertises `fs.readTextFile` and
   `fs.writeTextFile` limited to that folder; writes show as a diff and need approval.
9. **What Kubyl keeps.** Recommended: Kubyl doesn't persist transcripts (they hold logs and
   object YAML). `state.json` keeps per thread only the agent, cluster, title, cwd and the
   agent's session id, so agents with `loadSession` can reopen it from their own storage. Without
   `loadSession`, a closed thread is gone, and the panel says so.

## Secrets and tokens (the rules every tool follows)

The README's rule "never log or persist tokens, refresh tokens or Secret data" extends to "never
send them to an agent".

- **One masking module** for everything that leaves Kubyl: `kubyl_resources_core::redact` (a
  shared-crate commit moves `mask_secret` out of `kubyl_explorer::actions`, next to
  `route::mask_inline_key`; Helm's `present::mask_secrets` keeps its own YAML-level masking).
  It masks:
  - string values under credential-like keys anywhere in an object (`password`, `dbPassword`,
    `authToken`, `clientSecret`, `apiKey`…; `mask_credential_fields`) and the `value` of env
    vars with such names (`DB_PASSWORD`), since custom resources carry inline credentials;
  - core `v1` Secret `data` and `stringData`, and `kubectl.kubernetes.io/last-applied-configuration`
    on Secrets;
  - Route inline TLS keys;
  - Helm release Secrets and ConfigMaps (`owner=helm`): never returned, only the release summary
    phase 12 shows;
  - `managedFields` (noise, and they can echo values): dropped.
- **Kubyl never reveals for the agent.** The details' "Reveal" is for the user's eyes. There is
  no tool, setting or prompt that returns unmasked Secret data.
- **Token shapes in text** (logs, events, annotations, ConfigMap values, terminal output):
  best-effort scrubbing of JWTs, `Bearer …`/`Authorization:` values, OpenShift `sha256~…` tokens,
  and PEM private keys, replaced with `••••••••`. It's a safety net, not a guarantee; the panel's
  first-run note says that logs go to the agent as they are.
- **No credentials in tools.** Tools call the API through the thread's cluster client from
  `ConnectionManager`, so the API server applies the user's RBAC; no tool returns a kubeconfig,
  a token, a header, or `ClusterCaps` fields that hold URLs with userinfo
  (`client::redact_userinfo`).
- **Nothing from a thread goes to `tracing`**: not prompts, not tool results, not agent stderr
  (kept in a per-agent in-memory ring buffer the user can open as "Agent output"). `Debug` on
  thread types prints sizes, not contents.
- **The MCP endpoint** listens on loopback only, checks the bearer token in constant time, and
  stops with its thread. Requests without the token get 401 and nothing else.

## Tools (the `kubyl` MCP server)

Every tool works on the thread's cluster, returns at most 64 KiB (truncated with a note saying how
to narrow the call), and shows in the thread as a tool-call card naming the object, so the user
sees what was read.

### Part 1: read

- `cluster_info`: name, version, distribution, flags (PROD, read-only), the namespaces the user
  can list, and what Kubyl has open (the selected object, the active view).
- `list_resources(kind, namespace?, label_selector?, field_selector?, limit?)`: from
  `ResourceStores` when the kind is watched, else a list call; the same columns the explorer
  shows.
- `get_resource(kind, namespace?, name)`: masked YAML.
- `describe(kind, namespace?, name)`: the object, its owners and owned objects, recent events,
  and for workloads their pods and container states (like `kubectl describe`, masked).
- `events(namespace?, involved_object?, since?)`: phase 07's events, OOM kills included.
- `logs(namespace, pod, container?, previous?, since?, tail?, grep?)`: phase 05's log client,
  default tail 500 lines, scrubbed.
- `top(kind: pods | nodes, namespace?)`: CPU and memory usage from metrics-server, like
  `kubectl top` (Prometheus data goes through `query_prometheus`).
- `query_prometheus(promql, range?)`: phase 18's client when a Prometheus was found.
- `alerts(namespace?)`: phase 14's firing and pending alerts when Alertmanager was found.
- `can_i(verb, resource, namespace?)`: a SelfSubjectAccessReview.
- `api_resources()`: discovery, so the agent can name CRDs.

### Part 2: propose (write)

- `propose_apply(yaml)`: opens phase 04's diff against the live objects; the user applies
  (server-side apply, field manager `kubyl`) or rejects. Returns applied, rejected or the API
  error.
- `propose_patch(kind, namespace, name, patch, patch_type)`, `propose_delete(…)`,
  `propose_scale(…)`, `propose_restart(…)`: a summary card with the same flow, using the
  explorer's existing actions.
- Not offered on read-only clusters (`tools/list` leaves them out). PROD clusters add the usual
  confirmation. "Always allow" doesn't exist for writes.

## Permission policy (`kubyl_agent_core::policy`)

- `session/request_permission` shows a prompt in the thread with the agent's options (allow once,
  allow always, reject). "Allow always" is remembered for the thread only, never across threads.
- `terminal/create`: every command needs approval, shown with its cwd and env changes.
  Commands that read kubeconfigs or Secrets (`kubectl get secret`, `-o yaml` on Secrets,
  `cat ~/.kube/*`, `$KUBECONFIG`, `kubectl config view --raw`) get a warning line in the prompt.
  Their output is shown with phase 05's terminal renderer and scrubbed before it goes back.
- `fs/*`: reads inside the thread's folder are allowed; reads outside it and every write ask;
  writes show as a diff.
- `elicitation/create`: rendered as a small form; URL elicitations open in the system browser
  after a confirmation that shows the URL.
- Kubyl's own tools (`mcp__kubyl__<tool>` and the other spellings in
  `thread::is_kubyl_tool`, exact names only) are allowed without a prompt: they're read-only and
  masked. Kubyl picks the agent's "allow always" option so it stops asking.

## Tasks

### Shared-crate commits (each lands on its own, first)
- [x] `kubyl_resources_core::redact`: `mask_secret` moved from `kubyl_explorer::actions`, plus `mask_object` (`managedFields`, Route keys, credential-like fields), `is_helm_release` and token-shape scrubbing (`scrub_text`), with tests; the explorer's clipboard copy calls it unchanged. Helm's `present::mask_secrets` keeps its own YAML-level masking
- [x] `kubyl_core::actions::AskAgent` (cluster, label, uri, text), dispatched by other crates and handled by `kubyl_agent`; `kubyl_logs` dispatches it for selected lines ("Logs: Ask Agent About Selected Lines", `shift-a`)
- [x] `kubyl` crate: the `mcp-bridge` subcommand (checked before logging and GPUI start) and the `kubyl_agent::init` line
- [x] Workspace: `agent-client-protocol-schema =1.10.2` and `getrandom` (see decision 2), `cargo deny check` green

### ACP client and agents (`kubyl_agent_core::{acp, agents, jsonrpc}`)
- [x] Agent discovery: built-in list (commands from the ACP registry, 2026-10-07), settings, the login shell's `PATH`; install hint per agent; Windows `.cmd` shims through `cmd /C`. No version probe
- [x] Start an agent with `tokio::process`: stdio JSON-RPC, stderr into a 64 KiB ring buffer, `KUBECONFIG` per decision 6, `kill_on_drop`, stopped with its last thread. One process per agent, several sessions over it
- [x] `initialize` with Kubyl's capabilities (`fs`, `terminal`, `elicitation` form and URL); keeps the agent's (`loadSession`, `mcpCapabilities`, prompt capabilities, auth methods)
- [x] `authenticate`: the agent's "agent" methods as buttons, the agent's sign-in hint otherwise, Retry
- [x] Sessions: `session/new` with cwd and the `kubyl` MCP server, `session/prompt` with text and embedded context (links inline when the agent takes none), `session/cancel`, `session/set_mode`, `session/load` when offered. Prompt answers arrive in order after the turn's updates (`Peer::send_ordered`)
- [x] Thread model (`thread::Transcript`): messages, thoughts, tool calls with their updates, plans, modes, commands, titles, usage; unit-tested with JSON updates
- [x] Client methods: `fs/*` within the thread folder (outside it and every write ask), `terminal/*` as local processes with captured output (`terminal.rs`, not `kubyl_terminal_core`, which is pod exec), `session/request_permission` and `elicitation/create` through the policy

### MCP server and tools (`kubyl_agent_core::{mcp, tools}`)
- [x] Loopback server per thread, bearer token, streamable HTTP; the stdio bridge
- [x] Part 1 read tools (11), each with masking, size caps and tests; live test against `kubyl-dev`
- [x] Tool-call cards: Kubyl's calls are recognised by tool name and their arguments (`thread::KubylCall`), not `_meta`

### UI (`kubyl_agent`)
- [x] Mockup board 19 in `design/mockups/generate.py` (three screens). Not republished to the claude.ai artifact yet
- [x] Agent panel as a right `DockPanel`: thread list (open and reopenable), agent picker, cluster chip, markdown (gpui-component's `TextView`), tool-call cards, command output, plans, permission prompts, agent questions (forms, links), mode picker, cancel, input with context chips
- [x] `@` mentions of resources in the input: objects of the thread's cluster from Kubyl's watch caches (like the palette's object search; `mentions::rank`), picked with a click or Enter, attached as a masked chip
- [x] Session settings the agent offers (ACP `configOptions`, `session/set_config_option`): model, model settings, thinking level and mode as dropdowns (grouped choices kept), on/off settings as toggles; agents with only the older session modes get a mode dropdown. Kubyl advertises `session.configOptions.boolean`. Context use in percent in the thread header (`usage_update`; yellow from 80 %, red from 95 %, tokens and cost in the tooltip)
- [x] "Ask agent" on resource lists (`shift-a`, and wherever the selection is an object) and on selected log lines
- [x] "Ask agent" on events (the Events panel and view send the events the filter shows, at most 60), an alert (the details pane, `Alert::agent_text`) and an Argo CD app (the app header, `Application::agent_text`: status, sources, destination, conditions, the last operation, resources not synced or healthy)
- [x] Palette: `Agent: New Thread`, `Resource: Ask Agent`, `Agent: Show Agent Output`, `View: Agent Panel` (`secondary-?`)
- [x] Status bar item while an agent works or waits for the user; a toast when a turn ends while Kubyl isn't focused
- [x] Empty states: no agent found (install commands), agent needs sign-in, agent stopped (stderr tail, Retry, Agent output)
- [x] First-run note: what the agent can see, that logs go as they are, that the agent's own tools aren't sandboxed

### Part 2: writes
- [ ] `propose_*` tools through phase 04's diff and apply and the explorer's actions; hidden on read-only clusters, PROD confirmation
- [x] `fs/write_text_file` diffs in the panel (part 1 needed them for every write)

### Settings (`"agent"` section of settings.json)
- [x] `agent.default`, `agent.custom: [{id, name, command, args, env}]`, `agent.kubectl: "none" | "context"`, `agent.cwd` (empty: scratch folder), `agent.max_tool_output_kib`, `agent.default_log_lines`. `agent.tools.writes` comes with part 2

### Dev setup and tests
- [x] A scripted fake ACP agent (`acp::fake`, in-process over in-memory pipes rather than a binary): says text, asks permission, runs a command, reads and writes files, asks questions, calls Kubyl's tools over HTTP. The core's service tests drive it on `TestHost`
- [x] GPUI tests of the panel (`panel::gpui_tests`): the welcome, `kubyl::AskAgent` attaching a masked chip and starting a thread, commands and forms waiting for answers (validation included), `@` mentions. They use `test-support` hooks of the core (`set_program_for_tests`, `push_pending_for_tests`) instead of agent processes
- [x] Masking tests: Secrets, Helm releases, Route keys, credential fields, JWTs, bearer tokens, PEM keys, cloud and Git tokens, URL credentials
- [x] Live test on `kubyl-dev` (`tests/live.rs`): every read tool through the MCP server over HTTP, a planted Secret never returned
- [x] Protocol checks against the real adapters, without prompts (`tests/adapters.rs`, `#[ignore]`, local only): for every installed built-in agent, `initialize`, `session/new` with Kubyl's MCP server, the settings it offers, `session/set_config_option` with the current value and `session/cancel`. No model is called, so no tokens are spent; a sign-in request passes with a note. Passes against `claude-agent-acp` 0.86
- Decided: no tests send prompts to real agents, locally or in CI (they cost tokens and money, and CI has neither the adapters nor sign-ins). What happens inside a turn is tested with the scripted fake agent

## Acceptance criteria

- With Claude, Codex or Gemini installed and signed in, "Ask agent" on a CrashLoopBackOff pod
  starts a thread on that cluster; the agent calls `describe` and `logs` and explains the crash.
- No Secret value, token or private key reaches the agent through Kubyl, checked by the fake
  agent tests.
- Every shell command and every write is visible and needs a click; nothing is applied to a
  cluster without the diff or summary being approved; read-only clusters offer no write tools.
- A missing agent, a signed-out agent and a crashed agent each show what to do.
- No network or blocking call on the UI thread; closing Kubyl stops every agent process and MCP
  endpoint.

## Risks

- **The agent isn't sandboxed.** It can read the user's files and run its own tools. Decision 6
  and the first-run note are the mitigation; we must not claim more.
- **Moving targets.** ACP 2.x, the adapters and the agents' flags change often. Pin the crate,
  keep the built-in agent list data-only, and test against the fake agent in CI.
- **Agents differ.** Some don't route shell commands through `terminal/*`, some lack HTTP MCP or
  `loadSession`. Test each and record what works in the handoff log.
- **Prompt injection.** Logs, annotations and CRD fields are attacker-controlled text that the
  agent reads. Writes needing approval is the guard; the diff must show exactly what will be
  applied.
- **Cost and data.** Everything the agent reads goes to its provider. The first-run note says so;
  tool cards show what was read.

## Later (not in this phase)

- Installing agents from the ACP registry (downloads code: needs consent and checksums).
- Threads across clusters; a "compare clusters" tool.
- A plain local terminal tab with `KUBECONFIG` set to the active context (the other half of
  Lens's "smart terminal").
- Kubyl as an ACP **agent** or MCP server for other editors (Zed could use Kubyl's cluster tools).

## Handoff log

### 2026-10-07 (branch `phase/21-agents-acp`)

**Shipped (part 1).** `kubyl_agent_core`: `jsonrpc` (newline-delimited JSON-RPC, ordered
responses), `acp` (process, `initialize`, sessions, prompts, the router that answers terminal
and in-folder file requests on Tokio), `agents`, `settings`, `kubeconfig` (`none` / one-context
kubeconfig), `mcp` (loopback server, bearer token, `kubyl mcp-bridge`), `tools` (11 read
tools), `policy` (command warnings, execute grants, folder checks), `terminal` (local commands),
`thread` (transcript), `elicitation` (forms and links), `service` (`AgentCore` on `Host`).
`kubyl_agent`: service entity with per-cluster tool contexts (connection, selection, title-bar
namespace, Prometheus from `MetricsService`, alerts from `AlertsService`, refreshed on events
and every 5 s), the panel, actions, status item, output dialog.

**Decisions made here** (README table updated): decision 2 as written above; one MCP server per
thread; commands the agent runs through Kubyl are local processes (`sh -c` / `cmd /C`) with the
login shell's `PATH` and the thread's `KUBECONFIG`; a command is allowed without a second
prompt only when the user just allowed the agent's own "execute" permission for it (30 s,
same command) and it has no warning; Kubyl's own tools never prompt; thread folders live under
`<cache dir>/kubyl/agent/threads/` and are removed with the thread; state.json keeps reopenable
threads (agent, cluster, title, folder, session id), never transcripts.

**Verified.** `cargo fmt`, `clippy -D warnings` (workspace), `cargo test` of the touched crates
(40 core tests incl. end-to-end service tests with the fake agent), `cargo deny check`,
`script/check-core-crates.sh`, the live test against `kubyl-dev`. The user ran the panel with
Claude (`claude-agent-acp`) against a real cluster: that run found that Kubyl's own tools
were prompted for (fixed: never prompted now) and that a custom resource's spec leaked
inline credentials (`-DatabasePassword="…"`, `…AuthToken=…`) through `get_resource` (fixed:
`mask_credential_fields` and a broader scrub rule, with tests). Tool output that agents wrap
in a lone code fence now shows without the fence; mixed text renders as markdown.

**Not done / next.** Part 2 (`propose_*` writes); republishing the mockup artifact with
board 19; a version probe of installed agents. Masking of free text
stays best-effort: values under unusual key names in custom resources can still reach the
agent, which the first-run note says.

**Gotchas.** The agent's prompt response must not overtake its last `session/update`s: send
prompts with `Peer::send_ordered` and handle the end as `ClientCall::TurnEnded`. GUI-subsystem
Windows builds still get stdio pipes for `kubyl mcp-bridge`, but keep anything from writing to
stdout before the bridge runs (it's checked before logging starts).

### 2026-10-07, later (branch `phase/21-agents-acp`)

**Shipped.** "Ask agent" on events (a toolbar button of the Events panel and view), on an alert
(details pane) and on an Argo CD app (app header), all through `kubyl::AskAgent`; the text
builders `Alert::agent_text` and `Application::agent_text` live in the core crates with tests.
`@` mentions in the composer. GPUI tests of the panel. `AgentService` no longer needs a
`ConnectionManager` to start a thread (tests run without one).

**Not done / next.** Part 2 (`propose_*`); republishing the
mockup artifact (board 19 doesn't show the new "Ask agent" buttons in Events, Alerts and Argo
CD); mentions only offer objects Kubyl already watches (no server-side search).

### 2026-10-07, settings and context use

**Shipped.** Model, thinking level, mode and other session settings from the agent's
`configOptions` (`thread::ConfigOption`, `AgentCore::set_config`), and context use in percent
from `usage_update` (`Transcript::usage_percent`, cost when reported). Checked against
`claude-agent-acp` 0.86 with a protocol probe (`initialize` + `session/new`, no prompt): it offers
`mode` (Manual, Accept edits, Plan, Auto, Bypass permissions), `model` (Default, Opus 5.5,
Sonnet 5.5, Fable 5.1, Haiku 4.5…), `effort` as `thought_level` (Default to Max) and `fast` as a
boolean `model_config` (sent because Kubyl advertises boolean settings). Whether it sends
`usage_update` wasn't checked (that needs a prompt). Agents that report neither show nothing
extra. Choosing "Bypass permissions" stops the agent's own prompts, but commands Kubyl runs
and file changes still ask.

### 2026-10-07, adapter protocol checks

**Decided.** Tests never send a prompt to a real agent, locally or in CI: prompts cost tokens and
money. `tests/adapters.rs` (ignored, local only) checks the protocol of every installed
built-in adapter without calling a model: `initialize`, `session/new` with Kubyl's MCP server
(adapters without HTTP MCP get a bridge path that doesn't exist and must still start the
session), the session settings, `session/set_config_option` with the current value, and
`session/cancel`. `KUBYL_TEST_AGENTS=claude,codex` limits the run. Passes against
`claude-agent-acp` 0.86 (model, fast mode, effort, mode; five modes); Codex, Gemini CLI,
Copilot, goose and OpenCode weren't installed. Turns are tested with the fake agent only.

### 2026-10-07, CodeRabbit review of PR #40

**Fixed.** Exec plugins whose environment or arguments carry credentials aren't copied into the
`agent.kubectl: context` kubeconfig (`redact::is_credential_name`, `scrub_text` on the args).
Arguments of agent commands are quoted unless plain (POSIX single quotes; double quotes for
`cmd /C`), so shell syntax in them reaches the program literally. Command grants match the
command exactly (or the command inside `sh|bash|zsh -c`), and a permission prompt that showed no
command grants nothing. The agent's environment for a command can't replace `KUBECONFIG`
(dropped, with a warning), is shown in the prompt, and always needs a prompt. Saved thread
titles are scrubbed before state.json. Argo CD sources go to the agent without URL credentials
(`repo_short`). The MCP accept loop backs off after errors. The explorer's Copy YAML adds
`kind`/`apiVersion` to store objects before masking (a Secret without them wasn't masked).


- **Host hooks (library only, defaults unchanged).** `acp::Launch` has `cli_env`
  (`kubyl_kube_core::cli::CliEnv`: inherit everything, or an allow-list plus fixed variables),
  `capabilities` (`acp::ClientCapabilities`: terminal, fs read/write, elicitation; one that is off
  isn't advertised and its requests get method-not-found) and `session_meta` (`_meta` of
  `session/new` and `session/load`). `AgentCore` has `set_cli_env`, `set_client_capabilities`,
  `set_session_meta` (for agents started afterwards; commands the agent runs get the same
  `cli_env`), and the public `notice(thread, text, error, host)` and `set_program(agent,
  program, host)`, which `transcript_for_tests`' notice use and `set_program_for_tests` wrap
  or mirror. Tests: `acp::tests` (capabilities, `_meta`), `tests/launch_env.rs`, `service::tests`.
