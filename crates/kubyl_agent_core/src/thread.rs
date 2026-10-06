//! A conversation as the panel shows it, built from ACP `session/update`s.
//!
//! Pure data: [`Transcript::apply`] folds one update into the entries (message chunks merge,
//! tool calls update in place), so the panel and tests see the same thing. Transcripts live in
//! memory only; Kubyl never writes them to disk.

use agent_client_protocol_schema::v1::{
    ContentBlock, EmbeddedResourceResource, PlanEntryStatus, SessionConfigKind,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions, SessionUpdate,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdateFields, ToolKind,
};
use serde_json::Value;

/// Tool names of Kubyl's MCP server, to recognise its calls in the agent's tool list.
pub const KUBYL_TOOLS: &[&str] = &[
    "cluster_info",
    "list_resources",
    "get_resource",
    "describe",
    "events",
    "logs",
    "top",
    "query_prometheus",
    "alerts",
    "can_i",
    "api_resources",
];

/// Whether `name` is exactly one of Kubyl's own tools as an agent names it:
/// `mcp__kubyl__describe` (Claude, Gemini), `kubyl__describe`, `kubyl.describe`,
/// `kubyl/describe` or `kubyl_describe`. Only exact names count, so another tool that merely
/// mentions Kubyl isn't taken for one of these read-only tools.
pub fn is_kubyl_tool(name: &str) -> bool {
    let name = name.trim();
    let rest = name
        .strip_prefix("mcp__kubyl__")
        .or_else(|| name.strip_prefix("kubyl__"))
        .or_else(|| name.strip_prefix("kubyl."))
        .or_else(|| name.strip_prefix("kubyl/"))
        .or_else(|| name.strip_prefix("kubyl_"));
    rest.is_some_and(|tool| KUBYL_TOOLS.contains(&tool))
}

/// What a session setting is about, as the agent labels it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigCategory {
    Model,
    /// More settings of the model (e.g. a context size).
    ModelConfig,
    /// How much the model thinks (reasoning effort).
    ThoughtLevel,
    Mode,
    Other(String),
}

impl ConfigCategory {
    /// Order in the panel: model, thinking, mode, the rest.
    pub fn order(&self) -> u8 {
        match self {
            Self::Model => 0,
            Self::ModelConfig => 1,
            Self::ThoughtLevel => 2,
            Self::Mode => 3,
            Self::Other(_) => 4,
        }
    }
}

/// One choice of a select setting.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigChoice {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
    /// The group's name, for grouped choices (e.g. a provider).
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigValue {
    Select {
        current: String,
        choices: Vec<ConfigChoice>,
    },
    Bool(bool),
}

/// A session setting the agent offers (`configOptions`).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: ConfigCategory,
    pub value: ConfigValue,
}

impl ConfigOption {
    /// The current choice's name (`Opus 4`), `on`/`off` for booleans.
    pub fn current_label(&self) -> String {
        match &self.value {
            ConfigValue::Select { current, choices } => choices
                .iter()
                .find(|c| &c.value == current)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| current.clone()),
            ConfigValue::Bool(on) => if *on { "on" } else { "off" }.into(),
        }
    }
}

/// The settings in the agent's wire form, sorted for the panel (model first). Kinds Kubyl
/// doesn't know are left out.
pub fn config_options(options: &[SessionConfigOption]) -> Vec<ConfigOption> {
    let mut out: Vec<ConfigOption> = options
        .iter()
        .filter_map(|o| {
            let category = match &o.category {
                Some(SessionConfigOptionCategory::Model) => ConfigCategory::Model,
                Some(SessionConfigOptionCategory::ModelConfig) => ConfigCategory::ModelConfig,
                Some(SessionConfigOptionCategory::ThoughtLevel) => ConfigCategory::ThoughtLevel,
                Some(SessionConfigOptionCategory::Mode) => ConfigCategory::Mode,
                Some(SessionConfigOptionCategory::Other(other)) => {
                    ConfigCategory::Other(other.clone())
                }
                Some(_) | None => ConfigCategory::Other(String::new()),
            };
            let value = match &o.kind {
                SessionConfigKind::Select(select) => {
                    let choice =
                        |c: &agent_client_protocol_schema::v1::SessionConfigSelectOption,
                         group: Option<&str>| ConfigChoice {
                            value: c.value.0.to_string(),
                            name: c.name.clone(),
                            description: c.description.clone(),
                            group: group.map(str::to_string),
                        };
                    let choices = match &select.options {
                        SessionConfigSelectOptions::Ungrouped(options) => {
                            options.iter().map(|c| choice(c, None)).collect()
                        }
                        SessionConfigSelectOptions::Grouped(groups) => groups
                            .iter()
                            .flat_map(|g| g.options.iter().map(|c| choice(c, Some(&g.name))))
                            .collect(),
                        _ => Vec::new(),
                    };
                    ConfigValue::Select {
                        current: select.current_value.0.to_string(),
                        choices,
                    }
                }
                SessionConfigKind::Boolean(boolean) => ConfigValue::Bool(boolean.current_value),
                _ => return None,
            };
            Some(ConfigOption {
                id: o.id.0.to_string(),
                name: o.name.clone(),
                description: o.description.clone(),
                category,
                value,
            })
        })
        .collect();
    out.sort_by_key(|o| o.category.order());
    out
}

/// Something the user attached to a prompt ("Ask agent" on an object, log lines, an alert).
#[derive(Clone, Debug, PartialEq)]
pub struct ContextChip {
    /// Shown on the chip: `Pod shop/web-0`, `42 log lines`, `KubePodCrashLooping`.
    pub label: String,
    /// `kubyl://<cluster>/<kind>/<namespace>/<name>` (or `…/logs/…`, `…/alerts/…`).
    pub uri: String,
    /// Masked text the agent gets with the prompt.
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    Running,
    Done,
    Failed,
}

impl ToolStatus {
    fn of(status: ToolCallStatus) -> Self {
        match status {
            ToolCallStatus::Pending => Self::Pending,
            ToolCallStatus::InProgress => Self::Running,
            ToolCallStatus::Completed => Self::Done,
            ToolCallStatus::Failed => Self::Failed,
            _ => Self::Running,
        }
    }
}

/// What a tool call shows.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolContent {
    Text(String),
    Diff {
        path: String,
        old: Option<String>,
        new: String,
    },
    /// Output of a command Kubyl runs (`terminal/*`), read live from the agent's terminals.
    Terminal(String),
}

/// A call of one of Kubyl's own tools, recognised in a tool call.
#[derive(Clone, Debug, PartialEq)]
pub struct KubylCall {
    pub tool: String,
    pub arguments: Value,
}

impl KubylCall {
    /// `pod shop/web-0`, `pods in shop`, … for the card.
    pub fn summary(&self) -> String {
        let arg = |key: &str| self.arguments[key].as_str().unwrap_or_default().to_string();
        let object = || {
            let (kind, ns, name) = (arg("kind"), arg("namespace"), arg("name"));
            match (ns.is_empty(), name.is_empty()) {
                (_, true) => kind,
                (true, false) => format!("{kind} {name}"),
                (false, false) => format!("{kind} {ns}/{name}"),
            }
        };
        match self.tool.as_str() {
            "get_resource" | "describe" => object(),
            "list_resources" => {
                let ns = arg("namespace");
                if ns.is_empty() {
                    arg("kind")
                } else {
                    format!("{} in {ns}", arg("kind"))
                }
            }
            "logs" => format!("{}/{}", arg("namespace"), arg("pod")),
            "events" => {
                let ns = arg("namespace");
                if ns.is_empty() {
                    "all namespaces".into()
                } else {
                    ns
                }
            }
            "query_prometheus" => arg("query"),
            "can_i" => format!("{} {}", arg("verb"), arg("kind")),
            "top" => arg("kind"),
            _ => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolEntry {
    pub id: String,
    pub title: String,
    pub kind: Option<ToolKind>,
    pub status: ToolStatus,
    pub content: Vec<ToolContent>,
    pub raw_input: Option<Value>,
    pub kubyl: Option<KubylCall>,
}

impl ToolEntry {
    /// The shell command the call shows, if it's one (for permission grants).
    pub fn command(&self) -> Option<String> {
        let input = self.raw_input.as_ref()?;
        input["command"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                input["command"].as_array().map(|parts| {
                    parts
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .or_else(|| input["cmd"].as_str().map(str::to_string))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    User {
        text: String,
        chips: Vec<ContextChip>,
    },
    Agent {
        text: String,
        message_id: Option<String>,
    },
    Thought {
        text: String,
    },
    Tool(ToolEntry),
    Notice {
        text: String,
        error: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanItem {
    pub text: String,
    pub done: bool,
    pub active: bool,
}

/// The entries of one conversation, and what the agent reported alongside.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transcript {
    pub entries: Vec<Entry>,
    pub plan: Vec<PlanItem>,
    /// Slash commands the agent offers.
    pub commands: Vec<(String, String)>,
    pub mode: Option<String>,
    /// A title the agent set.
    pub title: Option<String>,
    /// Context window use (used, size), when the agent reports it.
    pub usage: Option<(u64, u64)>,
    /// The session's cost so far (amount, currency), when the agent reports it.
    pub cost: Option<(f64, String)>,
    /// The session's settings the agent offers (model, thinking level, mode…).
    pub config: Vec<ConfigOption>,
}

/// The text of a content block (other kinds become a short placeholder).
pub fn block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::ResourceLink(link) => format!("[{}]({})", link.name, link.uri),
        ContentBlock::Resource(resource) => match &resource.resource {
            EmbeddedResourceResource::TextResourceContents(text) => text.text.clone(),
            _ => "[binary resource]".into(),
        },
        ContentBlock::Image(_) => "[image]".into(),
        ContentBlock::Audio(_) => "[audio]".into(),
        _ => String::new(),
    }
}

fn kubyl_call(title: &str, name: Option<&str>, raw_input: Option<&Value>) -> Option<KubylCall> {
    let haystack = format!("{} {}", name.unwrap_or_default(), title).to_lowercase();
    if !haystack.contains("kubyl") {
        return None;
    }
    let tool = KUBYL_TOOLS
        .iter()
        // Longest first, so `list_resources` isn't taken for `logs`… (no overlaps today, but
        // keep it robust).
        .filter(|tool| haystack.contains(*tool))
        .max_by_key(|tool| tool.len())?;
    Some(KubylCall {
        tool: tool.to_string(),
        arguments: raw_input.cloned().unwrap_or(Value::Null),
    })
}

fn contents(content: &[ToolCallContent]) -> Vec<ToolContent> {
    content
        .iter()
        .filter_map(|c| match c {
            ToolCallContent::Content(content) => {
                let text = block_text(&content.content);
                (!text.is_empty()).then_some(ToolContent::Text(text))
            }
            ToolCallContent::Diff(diff) => Some(ToolContent::Diff {
                path: diff.path.display().to_string(),
                old: diff.old_text.clone(),
                new: diff.new_text.clone(),
            }),
            ToolCallContent::Terminal(terminal) => {
                Some(ToolContent::Terminal(terminal.terminal_id.0.to_string()))
            }
            _ => None,
        })
        .collect()
}

impl Transcript {
    /// How full the context window is, in percent (0–100), when the agent reports it.
    pub fn usage_percent(&self) -> Option<u8> {
        let (used, size) = self.usage?;
        (size > 0).then(|| ((used.saturating_mul(100) / size).min(100)) as u8)
    }

    /// Sets a setting's value locally (the agent's answer replaces the list).
    pub fn set_config(&mut self, id: &str, value: &ConfigValue) {
        if let Some(option) = self.config.iter_mut().find(|o| o.id == id) {
            match (&mut option.value, value) {
                (ConfigValue::Select { current, .. }, ConfigValue::Select { current: new, .. }) => {
                    *current = new.clone()
                }
                (ConfigValue::Bool(on), ConfigValue::Bool(new)) => *on = *new,
                _ => {}
            }
        }
    }

    pub fn push_user(&mut self, text: String, chips: Vec<ContextChip>) {
        self.entries.push(Entry::User { text, chips });
    }

    pub fn notice(&mut self, text: impl Into<String>, error: bool) {
        self.entries.push(Entry::Notice {
            text: text.into(),
            error,
        });
    }

    pub fn tool(&self, id: &str) -> Option<&ToolEntry> {
        self.entries.iter().rev().find_map(|e| match e {
            Entry::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut ToolEntry> {
        self.entries.iter_mut().rev().find_map(|e| match e {
            Entry::Tool(tool) if tool.id == id => Some(tool),
            _ => None,
        })
    }

    /// Folds one update in.
    pub fn apply(&mut self, update: SessionUpdate) {
        match update {
            SessionUpdate::UserMessageChunk(chunk) => {
                // Replayed history (`session/load`): merge into the last user entry.
                let text = block_text(&chunk.content);
                match self.entries.last_mut() {
                    Some(Entry::User { text: last, .. }) => last.push_str(&text),
                    _ => self.push_user(text, Vec::new()),
                }
            }
            SessionUpdate::AgentMessageChunk(chunk) => {
                let text = block_text(&chunk.content);
                let id = chunk.message_id.map(|m| m.0.to_string());
                match self.entries.last_mut() {
                    Some(Entry::Agent {
                        text: last,
                        message_id,
                    }) if id.is_none() || *message_id == id => last.push_str(&text),
                    _ => self.entries.push(Entry::Agent {
                        text,
                        message_id: id,
                    }),
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                let text = block_text(&chunk.content);
                match self.entries.last_mut() {
                    Some(Entry::Thought { text: last }) => last.push_str(&text),
                    _ => self.entries.push(Entry::Thought { text }),
                }
            }
            SessionUpdate::ToolCall(call) => self.tool_call(call),
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.0.to_string();
                match self.tool_mut(&id) {
                    Some(tool) => apply_fields(tool, update.fields),
                    None => {
                        // An update for a call we never saw: show what it says.
                        let mut tool = ToolEntry {
                            id,
                            title: String::new(),
                            kind: None,
                            status: ToolStatus::Running,
                            content: Vec::new(),
                            raw_input: None,
                            kubyl: None,
                        };
                        apply_fields(&mut tool, update.fields);
                        self.entries.push(Entry::Tool(tool));
                    }
                }
            }
            SessionUpdate::Plan(plan) => {
                self.plan = plan
                    .entries
                    .iter()
                    .map(|entry| PlanItem {
                        text: entry.content.clone(),
                        done: matches!(entry.status, PlanEntryStatus::Completed),
                        active: matches!(entry.status, PlanEntryStatus::InProgress),
                    })
                    .collect();
            }
            SessionUpdate::AvailableCommandsUpdate(update) => {
                self.commands = update
                    .available_commands
                    .iter()
                    .map(|c| (c.name.clone(), c.description.clone()))
                    .collect();
            }
            SessionUpdate::CurrentModeUpdate(update) => {
                self.mode = Some(update.current_mode_id.0.to_string());
            }
            SessionUpdate::SessionInfoUpdate(info) => {
                if let agent_client_protocol_schema::MaybeUndefined::Value(title) = info.title {
                    self.title = Some(title);
                }
            }
            SessionUpdate::UsageUpdate(usage) => {
                self.usage = Some((usage.used, usage.size));
                if let Some(cost) = usage.cost {
                    self.cost = Some((cost.amount, cost.currency));
                }
            }
            SessionUpdate::ConfigOptionUpdate(update) => {
                self.config = config_options(&update.config_options);
            }
            _ => {}
        }
    }

    fn tool_call(&mut self, call: ToolCall) {
        let kubyl = kubyl_call(&call.title, call.name.as_deref(), call.raw_input.as_ref());
        let entry = ToolEntry {
            id: call.tool_call_id.0.to_string(),
            title: call.title,
            kind: Some(call.kind),
            status: ToolStatus::of(call.status),
            content: contents(&call.content),
            raw_input: call.raw_input,
            kubyl,
        };
        match self.tool_mut(&entry.id) {
            Some(existing) => *existing = entry,
            None => self.entries.push(Entry::Tool(entry)),
        }
    }

    /// Marks running tool calls as failed (the turn ended or the agent went away).
    pub fn settle(&mut self) {
        for entry in &mut self.entries {
            if let Entry::Tool(tool) = entry
                && matches!(tool.status, ToolStatus::Pending | ToolStatus::Running)
            {
                tool.status = ToolStatus::Failed;
            }
        }
    }
}

fn apply_fields(tool: &mut ToolEntry, fields: ToolCallUpdateFields) {
    if let Some(title) = fields.title {
        tool.title = title;
    }
    if let Some(kind) = fields.kind {
        tool.kind = Some(kind);
    }
    if let Some(status) = fields.status {
        tool.status = ToolStatus::of(status);
    }
    if let Some(content) = fields.content {
        tool.content = contents(&content);
    }
    if let Some(raw_input) = fields.raw_input {
        tool.raw_input = Some(raw_input);
    }
    if tool.kubyl.is_none() {
        tool.kubyl = kubyl_call(&tool.title, fields.name.as_deref(), tool.raw_input.as_ref());
    } else if let (Some(kubyl), Some(input)) = (&mut tool.kubyl, &tool.raw_input) {
        kubyl.arguments = input.clone();
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn update(value: Value) -> SessionUpdate {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn chunks_merge_and_tool_calls_update_in_place() {
        let mut t = Transcript::default();
        t.push_user("why does web-0 crash?".into(), Vec::new());
        t.apply(update(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "Let me "}})));
        t.apply(update(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "look."}})));
        t.apply(update(json!({
            "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "mcp__kubyl__describe",
            "kind": "read", "status": "pending",
            "rawInput": {"kind": "pod", "namespace": "shop", "name": "web-0"},
        })));
        t.apply(update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "Name: web-0"}}]}),
        ));
        t.apply(update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "It runs "}})));
        t.apply(update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "out of memory."}})));

        assert_eq!(t.entries.len(), 4);
        assert_eq!(
            t.entries[1],
            Entry::Thought {
                text: "Let me look.".into()
            }
        );
        let Entry::Tool(tool) = &t.entries[2] else {
            panic!("{:?}", t.entries[2]);
        };
        assert_eq!(tool.status, ToolStatus::Done);
        assert_eq!(tool.content, [ToolContent::Text("Name: web-0".into())]);
        let kubyl = tool.kubyl.as_ref().unwrap();
        assert_eq!(kubyl.tool, "describe");
        assert_eq!(kubyl.summary(), "pod shop/web-0");
        assert_eq!(
            t.entries[3],
            Entry::Agent {
                text: "It runs out of memory.".into(),
                message_id: None
            }
        );
    }

    #[test]
    fn only_exact_kubyl_tool_names_count() {
        assert!(is_kubyl_tool("mcp__kubyl__cluster_info"));
        assert!(is_kubyl_tool("kubyl.describe"));
        assert!(!is_kubyl_tool("mcp__kubyl__delete_everything"));
        assert!(!is_kubyl_tool("Run kubyl describe in a shell"));
        assert!(!is_kubyl_tool("mcp__other__describe"));
    }

    #[test]
    fn plans_modes_commands_and_titles() {
        let mut t = Transcript::default();
        t.apply(update(json!({"sessionUpdate": "plan", "entries": [
            {"content": "Read the pod", "priority": "high", "status": "completed"},
            {"content": "Check events", "priority": "medium", "status": "in_progress"},
        ]})));
        t.apply(update(
            json!({"sessionUpdate": "current_mode_update", "currentModeId": "plan"}),
        ));
        t.apply(update(
            json!({"sessionUpdate": "available_commands_update", "availableCommands": [
            {"name": "compact", "description": "Compact the conversation"}]}),
        ));
        t.apply(update(
            json!({"sessionUpdate": "session_info_update", "title": "web-0 crash"}),
        ));
        assert!(t.plan[0].done && t.plan[1].active);
        assert_eq!(t.mode.as_deref(), Some("plan"));
        assert_eq!(t.commands[0].0, "compact");
        assert_eq!(t.title.as_deref(), Some("web-0 crash"));
    }

    #[test]
    fn settings_and_usage_from_updates() {
        let mut t = Transcript::default();
        t.apply(update(json!({"sessionUpdate": "config_option_update", "configOptions": [
            {"id": "effort", "name": "Thinking", "category": "thought_level", "type": "select",
             "currentValue": "medium", "options": [
                {"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]},
            {"id": "model", "name": "Model", "category": "model", "type": "select",
             "currentValue": "opus", "options": [
                {"group": "anthropic", "name": "Anthropic", "options": [
                    {"value": "opus", "name": "Opus"}, {"value": "sonnet", "name": "Sonnet"}]}]},
            {"id": "fast", "name": "Fast mode", "type": "boolean", "currentValue": false},
        ]})));
        assert_eq!(t.config.len(), 3);
        assert_eq!(t.config[0].category, ConfigCategory::Model);
        assert_eq!(t.config[0].current_label(), "Opus");
        let ConfigValue::Select { choices, .. } = &t.config[0].value else {
            panic!();
        };
        assert_eq!(choices[1].group.as_deref(), Some("Anthropic"));
        assert_eq!(t.config[1].category, ConfigCategory::ThoughtLevel);
        assert_eq!(t.config[2].current_label(), "off");
        t.set_config(
            "effort",
            &ConfigValue::Select {
                current: "high".into(),
                choices: Vec::new(),
            },
        );
        assert_eq!(t.config[1].current_label(), "High");

        assert_eq!(t.usage_percent(), None);
        t.apply(update(
            json!({"sessionUpdate": "usage_update", "used": 53000, "size": 200000,
            "cost": {"amount": 0.42, "currency": "USD"}}),
        ));
        assert_eq!(t.usage_percent(), Some(26));
        assert_eq!(t.cost, Some((0.42, "USD".to_string())));
    }

    #[test]
    fn commands_and_terminals_are_found() {
        let mut t = Transcript::default();
        t.apply(update(json!({
            "sessionUpdate": "tool_call", "toolCallId": "b", "title": "Run kubectl",
            "kind": "execute", "status": "in_progress", "rawInput": {"command": ["kubectl", "get", "pods"]},
            "content": [{"type": "terminal", "terminalId": "term-1"}],
        })));
        let tool = t.tool("b").unwrap();
        assert_eq!(tool.command().as_deref(), Some("kubectl get pods"));
        assert_eq!(tool.content, [ToolContent::Terminal("term-1".into())]);
        assert!(tool.kubyl.is_none());
        t.settle();
        assert_eq!(t.tool("b").unwrap().status, ToolStatus::Failed);
    }
}
