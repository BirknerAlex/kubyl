use gpui::{App, Hsla, IntoElement, Rems, RenderOnce, SharedString, Styled, Svg, Window, svg};

use crate::{ActiveColors, u};

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        /// Lucide icons bundled in `assets/icons/lucide` (ISC license). These are the icons the
        /// mockups use, plus a few for dock toggles.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum IconName {
            $($variant,)*
        }

        impl IconName {
            /// All icons, for galleries and tests.
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)*];

            /// Asset path, e.g. `icons/lucide/box.svg`.
            pub fn path(self) -> SharedString {
                match self {
                    $(IconName::$variant => SharedString::new_static(concat!("icons/lucide/", $file, ".svg")),)*
                }
            }
        }
    };
}

icons! {
    Activity => "activity",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    Bell => "bell",
    Blocks => "blocks",
    Box => "box",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    CircleCheck => "circle-check",
    CircleX => "circle-x",
    Clock => "clock",
    Cloud => "cloud",
    Code => "code",
    Columns => "columns-2",
    Copy => "copy",
    Cpu => "cpu",
    Database => "database",
    Diff => "diff",
    Download => "download",
    Ellipsis => "ellipsis",
    Eye => "eye",
    EyeOff => "eye-off",
    File => "file",
    FilePlus => "file-plus",
    Folder => "folder",
    Funnel => "funnel",
    Gauge => "gauge",
    Globe => "globe",
    HardDrive => "hard-drive",
    Info => "info",
    Key => "key",
    Layers => "layers",
    Link => "link",
    List => "list",
    Lock => "lock",
    Maximize => "maximize-2",
    Minimize => "minimize-2",
    Minus => "minus",
    Moon => "moon",
    Network => "network",
    PanelBottom => "panel-bottom",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    Pause => "pause",
    Play => "play",
    Plus => "plus",
    RefreshCw => "refresh-cw",
    Rows => "rows-2",
    Search => "search",
    Server => "server",
    Settings => "settings",
    Shield => "shield",
    ShipWheel => "ship-wheel",
    SlidersVertical => "sliders-vertical",
    Star => "star",
    StarFilled => "star-filled",
    Store => "store",
    Sun => "sun",
    Terminal => "terminal",
    Trash => "trash",
    TriangleAlert => "triangle-alert",
    Upload => "upload",
    User => "user",
    WrapText => "wrap-text",
    X => "x",
    Zap => "zap",
}

impl From<IconName> for SharedString {
    fn from(icon: IconName) -> Self {
        icon.path()
    }
}

/// An icon at a size and color. Defaults: 14px, dim text color.
#[derive(IntoElement)]
pub struct Icon {
    path: SharedString,
    size: Rems,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(icon: IconName) -> Self {
        Self::from_path(icon.path())
    }

    /// An icon from an asset path (for example one a [`kubyl_core::TabView`] returned).
    pub fn from_path(path: impl Into<SharedString>) -> Self {
        Self {
            path: path.into(),
            size: u(14.0),
            color: None,
        }
    }

    /// Size in unscaled pixels.
    pub fn size(mut self, px: f32) -> Self {
        self.size = u(px);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let svg: Svg = svg()
            .path(self.path)
            .flex_none()
            .size(self.size)
            .text_color(self.color.unwrap_or(cx.colors().text_dim));
        svg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AssetSource as _;

    #[test]
    fn every_icon_is_bundled() {
        for icon in IconName::ALL {
            let path = icon.path();
            assert!(
                crate::Assets.load(&path).unwrap().is_some(),
                "{path} is missing"
            );
        }
    }
}
