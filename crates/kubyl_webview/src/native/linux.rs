//! Linux: WebKitGTK through wry. Placeholder until the Linux backend lands (see the phase
//! 08 handoff log).

use futures::channel::mpsc::UnboundedSender;
use wry::WebViewBuilder;

use super::{NativeEvent, NativeOptions, ParentWindow};

pub struct Attached;

impl Attached {
    pub fn present(&self) {}

    pub fn set_title(&self, _: &str) {}
}

pub fn build(
    builder: WebViewBuilder<'_>,
    parent: &ParentWindow,
    _: &NativeOptions,
    _: &UnboundedSender<NativeEvent>,
) -> anyhow::Result<(wry::WebView, ())> {
    Ok((builder.build_as_child(parent)?, ()))
}

pub fn attach(
    _: &wry::WebView,
    _: (),
    _: &NativeOptions,
    _: UnboundedSender<NativeEvent>,
) -> anyhow::Result<Attached> {
    Ok(Attached)
}
