//! "Ask agent" on any resource (phase 25): canned questions about the selected object in the
//! list's menu, the palette and the object's details.
//!
//! The questions are in `kubyl_agent_core::prompts` and hold only a reference to the object;
//! the agent reads the rest through Kubyl's read-only tools. Clicking one opens the Agent panel
//! with the question in the composer: nothing is sent (and no tokens are spent) until the user
//! presses Enter. Everything is hidden while no agent is installed.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, Window, actions, prelude::*,
};
use kubyl_agent_core::prompts::{Prompt, Subject};
use kubyl_core::actions::AskAgentAbout;
use kubyl_core::{ActionRegistry, ActionSpec, ClusterCaps, DetailsSection, ResourceRef};
use kubyl_kube::ConnectionManager;
use kubyl_resources::ResourceSelection;
use kubyl_ui::{ActiveColors, Button, IconName, h_flex, u, v_flex};

use crate::service::AgentService;

actions!(
    agent_ask,
    [
        /// Asks the agent to summarize the selected object.
        AskSummarize,
        /// Asks the agent to analyze the selected object's events.
        AskEvents,
        /// Asks the agent to analyze the selected object's logs.
        AskLogs,
        /// Asks the agent to analyze the selected object's resource usage.
        AskMetrics,
        /// Asks the agent to analyze the resources related to the selected object.
        AskRelated,
    ]
);

/// Whether an agent is installed, for the availability predicates (which can't read the app).
/// Kept current by [`sync`].
static READY: AtomicBool = AtomicBool::new(false);

/// Whether the user has an agent to ask: at least one installed.
pub fn agent_ready(cx: &App) -> bool {
    AgentService::global(cx).is_some_and(|service| {
        service
            .read(cx)
            .agents()
            .iter()
            .any(|agent| agent.program.is_some())
    })
}

/// Mirrors [`agent_ready`] into the flag the action predicates read.
fn sync(cx: &App) {
    READY.store(agent_ready(cx), Ordering::Relaxed);
}

fn available(prompt: Prompt, target: &ResourceRef, _: &ClusterCaps) -> bool {
    READY.load(Ordering::Relaxed)
        && target.is_object()
        && prompt.applies_to(&target.gvr.group, &target.gvr.resource)
}

pub(crate) fn init(cx: &mut App) {
    if let Some(service) = AgentService::global(cx) {
        cx.observe(&service, |_, cx| sync(cx)).detach();
    }
    sync(cx);
    // One action per question, so the list's menu, the palette and key maps can name them.
    macro_rules! register {
        ($action:ident, $prompt:expr) => {
            ActionRegistry::register(
                cx,
                ActionSpec::new(format!("Ask Agent: {}", $prompt.label()), $action)
                    .hint(format!("Ask: {}", $prompt.label()))
                    .in_context(crate::RESOURCE_LIST)
                    .available_when(|target, caps| available($prompt, target, caps)),
            );
            cx.on_action(|_: &$action, cx| ask_selected($prompt, cx));
        };
    }
    register!(AskSummarize, Prompt::Summarize);
    register!(AskEvents, Prompt::Events);
    register!(AskLogs, Prompt::Logs);
    register!(AskMetrics, Prompt::Metrics);
    register!(AskRelated, Prompt::Related);
    cx.on_action(|ask: &AskAgentAbout, cx| {
        if let Some(prompt) = Prompt::from_id(&ask.prompt) {
            ask_about(&ask.target, &ask.kind, prompt, cx);
        }
    });
    kubyl_core::ChromeRegistry::add_details_section(cx, AskDetails);
}

/// The question for the object selected in the focused list.
fn ask_selected(prompt: Prompt, cx: &mut App) {
    let Some(selected) = ResourceSelection::global(cx).primary().cloned() else {
        return;
    };
    ask_about(&selected.target, &selected.kind, prompt, cx);
}

/// Puts `prompt` about `target` into the Agent panel's composer, when an agent is installed.
pub fn ask_about(target: &ResourceRef, kind: &str, prompt: Prompt, cx: &mut App) {
    let Some(name) = target.name.clone() else {
        return;
    };
    if !agent_ready(cx) || !prompt.applies_to(&target.gvr.group, &target.gvr.resource) {
        return;
    }
    let cluster_name = ConnectionManager::try_global(cx)
        .map(|m| m.read(cx).display_name(&target.cluster).to_string())
        .unwrap_or_else(|| target.cluster.to_string());
    let subject = Subject {
        cluster: cluster_name,
        kind: kind.to_string(),
        group: target.gvr.group.clone(),
        resource: target.gvr.resource.clone(),
        namespace: target.namespace.clone(),
        name,
    };
    let text = prompt.text(&subject);
    let cluster = target.cluster.clone();
    crate::with_window(cx, move |window, cx| {
        if let Some(panel) = crate::panel::show(window, cx) {
            panel.update(cx, |panel, cx| panel.prefill(cluster, &text, window, cx));
        }
    });
}

/// The details section: one button per question that applies.
pub struct AskDetails;

impl DetailsSection for AskDetails {
    fn id(&self) -> &'static str {
        "ask-agent"
    }

    fn order(&self) -> i32 {
        90
    }

    fn build(&self, target: &ResourceRef, kind: &str, cx: &mut App) -> Option<AnyView> {
        if !agent_ready(cx) || !target.is_object() {
            return None;
        }
        let prompts: Vec<Prompt> = Prompt::ALL
            .into_iter()
            .filter(|p| p.applies_to(&target.gvr.group, &target.gvr.resource))
            .collect();
        let (target, kind) = (target.clone(), kind.to_string());
        Some(
            cx.new(|_| AskSection {
                target,
                kind,
                prompts,
            })
            .into(),
        )
    }
}

struct AskSection {
    target: ResourceRef,
    kind: String,
    prompts: Vec<Prompt>,
}

impl Render for AskSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        v_flex()
            .px(u(14.0))
            .py(u(12.0))
            .gap(u(8.0))
            .border_b_1()
            .border_color(colors.border_variant)
            .child(
                gpui::div()
                    .text_size(u(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(colors.text_dim)
                    .child("ASK AGENT"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap(u(6.0))
                    .children(self.prompts.iter().map(|prompt| {
                        let (target, kind, id) = (
                            self.target.clone(),
                            self.kind.clone(),
                            prompt.id().to_string(),
                        );
                        Button::new(gpui::SharedString::from(format!("ask-{}", prompt.id())))
                            .ghost()
                            .icon(IconName::Zap)
                            .label(prompt.label())
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(
                                    Box::new(AskAgentAbout {
                                        target: target.clone(),
                                        kind: kind.clone(),
                                        prompt: id.clone(),
                                    }),
                                    cx,
                                )
                            })
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_follows_the_agent_and_the_kind() {
        let caps = ClusterCaps::default();
        let pod = ResourceRef::object(
            kubyl_core::ClusterId::new("c"),
            kubyl_core::Gvr::new("", "v1", "pods"),
            Some("shop".into()),
            "web".into(),
        );
        let list = ResourceRef::list(pod.cluster.clone(), pod.gvr.clone(), None);
        READY.store(false, Ordering::Relaxed);
        assert!(
            !available(Prompt::Summarize, &pod, &caps),
            "no agent: hidden"
        );
        READY.store(true, Ordering::Relaxed);
        assert!(available(Prompt::Logs, &pod, &caps));
        assert!(
            !available(Prompt::Summarize, &list, &caps),
            "a list isn't an object"
        );
        let config = ResourceRef::object(
            pod.cluster.clone(),
            kubyl_core::Gvr::new("", "v1", "configmaps"),
            Some("shop".into()),
            "cfg".into(),
        );
        assert!(available(Prompt::Summarize, &config, &caps));
        assert!(!available(Prompt::Logs, &config, &caps));
        READY.store(false, Ordering::Relaxed);
    }
}
