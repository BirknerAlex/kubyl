//! Status bar item: shows nothing until an update is available, then "Restart to update" once
//! it's downloaded and verified (Zed-style).

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, Render, Subscription, Window, div,
    prelude::*,
};
use kubyl_core::{StatusBarItem, StatusBarPosition};
use kubyl_ui::{ActiveColors, Icon, IconName, h_flex, u};

use crate::service::{SelfUpdate, UpdateState};

pub struct SelfUpdateStatusItem;

impl StatusBarItem for SelfUpdateStatusItem {
    fn id(&self) -> &'static str {
        "self-update"
    }

    fn position(&self) -> StatusBarPosition {
        StatusBarPosition::Right
    }

    fn build(&self, _: &mut Window, cx: &mut App) -> AnyView {
        cx.new(SelfUpdateStatusView::new).into()
    }
}

struct SelfUpdateStatusView {
    _subscription: Option<Subscription>,
}

impl SelfUpdateStatusView {
    fn new(cx: &mut Context<Self>) -> Self {
        let subscription =
            SelfUpdate::global(cx).map(|update| cx.observe(&update, |_, _, cx| cx.notify()));
        Self {
            _subscription: subscription,
        }
    }
}

impl Render for SelfUpdateStatusView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.colors().clone();
        let shown = SelfUpdate::global(cx).and_then(|update| {
            let update = update.read(cx);
            match update.state() {
                UpdateState::Idle | UpdateState::UpToDate => None,
                UpdateState::Checking => {
                    Some(("checking for updates…".to_string(), colors.text_dim, false))
                }
                UpdateState::Available(_) | UpdateState::Downloading(_) => {
                    Some(("downloading update…".to_string(), colors.text_dim, false))
                }
                UpdateState::ReadyToRestart(manifest) => Some((
                    format!("restart to update to v{}", manifest.version),
                    colors.green,
                    true,
                )),
                UpdateState::Failed(_) => {
                    Some(("update check failed".to_string(), colors.text_dim, false))
                }
            }
        });
        h_flex()
            .id("self-update")
            .gap(u(5.0))
            .when_some(shown, |this, (label, color, restart)| {
                this.when(restart, |this| {
                    this.cursor_pointer()
                        .on_click(|_, _, cx| SelfUpdate::restart(cx))
                })
                .child(Icon::new(IconName::Download).size(12.0).color(color))
                .child(div().text_color(color).child(label))
            })
    }
}
