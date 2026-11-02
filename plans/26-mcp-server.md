# Phase 26: Agent changes and Kubyl as an MCP server

**Status:** done (branch `feat/agent-writes-mcp-server`)
**Depends on:** 21 (agents over ACP, the read tools and the MCP server it started with)
**Owns:** `crates/kubyl_mcp_core` (new, GPUI-free), `crates/kubyl_mcp` (new); the MCP server (`mcp.rs`), the tools (`tools.rs`) and the JSON-RPC error/`handle_single` pieces move from `kubyl_agent_core` to `kubyl_mcp_core` (re-exported there under the old paths); small commits in `kubyl_agent_core` (write gate, approval) and `kubyl_agent` (switch, cards)
**Mockups:** none yet. The switch, the warning dialog and the approval dialog follow board 19.

## Goal

1. **Agent writes.** Phase 21's agents could only read. A switch in the thread's composer ("Allow changes") lets the agent *propose* changes; the user approves every one in Kubyl.
2. **Kubyl as an MCP server.** A localhost MCP server, on by default, port configurable, so tools such as Claude Code can use the running Kubyl and every cluster it is connected to.

## Design

See the README's decision rows "Agent changes" and "Kubyl as an MCP server". In short:

- `kubyl_mcp_core::{server, handler, tools, changes, approval, hub, settings, rpc}`. One `server::serve(handler, options)` for both uses; `handler::ToolsHandler` puts the tools behind it with a `Scope` (`Cluster`: a thread; `Clusters`: all) and optional `Writes` (the on/off watch, the `Approver`, the message for "off").
- Change tools: check (dry run) → `Proposal` (summary, warnings, masked diffs) → `Approver::approve` → act. Nothing acts without `Decision::Approved`.
- Approvers: a thread's `ChannelApprover` feeds `AgentCore::change_requested`, which adds a `PendingKind::Change` the panel renders as a card; the hub's feeds `McpHub::pending`, which `kubyl_mcp` shows as a dialog.
- `kubyl_mcp::McpService` wraps `McpHub` (settings observer, connection/selection events, 5 s refresh of the clusters' `ToolContext`s), plus the status bar item and the `MCP: …` actions.

## Tasks

- [x] `kubyl_mcp_core` crate: server (loopback only, bearer token, `Origin`/`Host` checks, fixed port also on `::1`), `ToolsHandler`, `list_clusters` and the `cluster` argument, settings, `McpHub` with the keychain token
- [x] Change tools: `apply_manifest` (dry run, diff, SSA, no force, no Secrets), `patch_resource` (merge/strategic/json), `delete_resource`, `scale_resource`, `restart_resource`; read-only clusters and an off switch refuse them
- [x] Thread write gate (`AgentCore::set_writes`, `Thread::writes_enabled`, off by default and after reopening), `PendingKind::Change`, `agent.allow_writes`
- [x] Panel: "Allow changes" switch with the warning dialog, "Changes on" in the thread header, change card with diff and Apply/Reject, PROD typed confirmation (`kubyl_mcp::view::ask`)
- [x] `kubyl_mcp`: service, status bar item, approval dialog for outside clients, `MCP: Show Server`, `MCP: Copy Claude Code Command`, `MCP: Copy Client Config (JSON)`, `MCP: Copy Token`, `MCP: Regenerate Token`
- [x] Tests: server auth/origin/host, bridge, handler (scopes, picking a cluster, gates), change tool checks, hub on a real port (token, port in use, approvals), thread approvals on `TestHost`, panel switch/card on GPUI
- [x] Change tools against a fake API server (`kubyl_mcp_core/tests/changes.rs`): approved scale/patch/delete are sent once (patches dry-run first), rejected and no-op changes send nothing, Secrets are refused
- [ ] Live test of the change tools against `kubyl-dev` (kind), including `apply_manifest` (needs a real API server for the dry run and server-side apply)
- [x] "MCP: Show Server" dialog: Run/Allow changes switches and the port, saved to settings.json (`Settings::update`), also under View → "MCP Server" (the status bar item is hidden while the server is off); status bar dot (green running, grey standby, red failed, yellow change waiting)
- [x] Several instances: the first serves the port, the others find a Kubyl with the same token there (`server::serves_kubyl`), wait on `ServerState::Standby` and retry every 3 s, so one takes over when the serving one quits
- [ ] Mockup for the switch, warning, approval and server dialogs

## Notes for the next session

- **Settings** are JSON only (no settings UI exists yet): `"mcp": { "enabled": true, "port": 7421, "writes": "off" | "ask" }` and `"agent": { "allow_writes": true }`.
- Agents cache the tool list, so the change tools are always listed and refuse while off, with a message that tells the agent what to ask the user.
- Claude Code: `claude mcp add --transport http kubyl http://127.0.0.1:7421/mcp --header "Authorization: Bearer <token>"` (copy it with the palette action; the token is in the OS keychain).
- Claude Desktop: the release ships `kubyl-<version>.mcpb` (job `build-mcpb`, source `packaging/mcpb`), a dependency-free Node relay from stdio to `http://127.0.0.1:<port>/mcp`; the user pastes the token from "Copy token". It uses `node:http` because Kubyl refuses requests with an `Origin`. Built locally with `npx @anthropic-ai/mcpb pack packaging/mcpb kubyl.mcpb` (git-ignored).
- Standby: "New token" is refused there (the serving instance would keep the old token); copying works, since the URL and keychain token are the same. Approvals show in the serving instance. Between the serving one quitting and the takeover (≤ 3 s) clients get "connection refused".
- `[::1]` is bound best-effort on the same port; if it is taken the server still runs on IPv4.
