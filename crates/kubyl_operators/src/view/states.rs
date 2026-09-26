//! What an OLM sub-tab shows instead of a list: not connected, loading, OLM not installed, or
//! what's missing to read it.

use std::sync::Arc;

use gpui::{AnyElement, Context, FontWeight, IntoElement, div, prelude::*};
use kubyl_ui::{ActiveColors, Button, Icon, IconName, fonts, h_flex, u, v_flex};

use super::{OperatorsView, SubTab};
use crate::service::{Availability, Snapshot};

/// Why a sub-tab can't show its list.
#[derive(Clone, Debug, PartialEq)]
pub enum Blocking {
    NotConnected,
    Loading,
    NoOlm,
    Problems(Vec<String>),
}

pub fn blocking(availability: Availability, snapshot: &Option<Arc<Snapshot>>) -> Option<Blocking> {
    match availability {
        Availability::NotConnected => Some(Blocking::NotConnected),
        Availability::NoOlm => Some(Blocking::NoOlm),
        Availability::Loading | Availability::Ready => match snapshot {
            None => Some(Blocking::Loading),
            Some(s) if !s.problems.is_empty() && s.operators.is_empty() && s.plans.is_empty() => {
                Some(Blocking::Problems(s.problems.clone()))
            }
            Some(s) if s.loading => Some(Blocking::Loading),
            Some(_) => None,
        },
    }
}

impl OperatorsView {
    pub(crate) fn render_state(&mut self, state: Blocking, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.colors().clone();
        match state {
            Blocking::NotConnected => centered(
                IconName::Blocks,
                "Connecting to the cluster…".into(),
                cx,
            ),
            Blocking::Loading => centered(IconName::Blocks, "Loading operators…".into(), cx),
            Blocking::Problems(problems) => v_flex()
                .size_full()
                .p(u(30.0))
                .gap(u(10.0))
                .child(
                    h_flex()
                        .gap(u(12.0))
                        .child(Icon::new(IconName::Lock).size(20.0).color(colors.yellow))
                        .child(
                            div()
                                .text_size(u(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child("Kubyl can't read OLM on this cluster"),
                        ),
                )
                .children(problems.into_iter().map(|p| {
                    div()
                        .text_size(u(12.5))
                        .text_color(colors.text_muted)
                        .child(p)
                }))
                .into_any_element(),
            Blocking::NoOlm => {
                v_flex()
                    .size_full()
                    .p(u(30.0))
                    .gap(u(14.0))
                    .child(
                        h_flex()
                            .gap(u(12.0))
                            .child(Icon::new(IconName::Blocks).size(22.0).color(colors.text_dim))
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .text_size(u(16.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("OLM isn't installed on this cluster"),
                                    )
                                    .child(
                                        div()
                                            .text_size(u(12.5))
                                            .text_color(colors.text_muted)
                                            .child("No operators.coreos.com or olm.operatorframework.io APIs. Helm releases work without it."),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .max_w(u(620.0))
                            .gap(u(6.0))
                            .p(u(12.0))
                            .rounded(u(8.0))
                            .bg(colors.panel)
                            .border_1()
                            .border_color(colors.border)
                            .text_size(u(12.5))
                            .text_color(colors.text_muted)
                            .child("OpenShift ships OLM. Other clusters install it from the operator-framework releases:")
                            .child(
                                div()
                                    .px(u(8.0))
                                    .py(u(6.0))
                                    .rounded(u(5.0))
                                    .bg(colors.background)
                                    .font_family(fonts::MONO)
                                    .text_size(u(11.5))
                                    .child("operator-sdk olm install   # or the release's install.sh"),
                            )
                            .child("Kubyl's dev cluster: script/olm-dev.sh (OLM, the operatorhub.io catalog and a subscription waiting for approval)."),
                    )
                    .child(
                        h_flex()
                            .gap(u(8.0))
                            .child(
                                Button::new("olm-show-helm")
                                    .icon(IconName::Anchor)
                                    .label("Show Helm releases")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_tab(SubTab::Helm, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("olm-docs")
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label("OLM documentation")
                                    .on_click(|_, _, cx| {
                                        crate::widgets::open_url("https://olm.operatorframework.io/docs/", cx)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(u(11.5))
                            .text_color(colors.text_dim)
                            .child("Kubyl looks again when the cluster's APIs change."),
                    )
                    .into_any_element()
            }
        }
    }
}

pub fn centered(icon: IconName, text: String, cx: &mut Context<OperatorsView>) -> AnyElement {
    let colors = cx.colors().clone();
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap(u(10.0))
        .text_color(colors.text_dim)
        .child(Icon::new(icon).size(26.0).color(colors.text_faint))
        .child(text)
        .into_any_element()
}
