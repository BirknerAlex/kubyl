use std::borrow::Cow;

use anyhow::Result;
use gpui::{App, AssetSource, SharedString};
use rust_embed::RustEmbed;

/// Assets compiled into the binary: fonts, Lucide icons and the logo PNGs.
///
/// Paths are relative to the repository's `assets/` directory, e.g. `icons/lucide/box.svg`.
/// Anything not found here is looked up in gpui-component's bundled assets.
#[derive(RustEmbed)]
#[folder = "../../assets"]
#[include = "fonts/**/*.ttf"]
#[include = "icons/**/*.svg"]
#[include = "logo/png/*.png"]
#[include = "logo/*.svg"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(file) = <Self as RustEmbed>::get(path) {
            return Ok(Some(file.data));
        }
        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries: Vec<SharedString> = <Self as RustEmbed>::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(p.into_owned()))
            .collect();
        entries.extend(gpui_kit_assets::Assets.list(path).unwrap_or_default());
        Ok(entries)
    }
}

/// Registers the bundled IBM Plex fonts with GPUI's text system.
pub fn load_fonts(cx: &mut App) {
    let fonts: Vec<Cow<'static, [u8]>> = <Assets as RustEmbed>::iter()
        .filter(|path| path.starts_with("fonts/") && path.ends_with(".ttf"))
        .filter_map(|path| <Assets as RustEmbed>::get(&path).map(|file| file.data))
        .collect();
    if let Err(err) = cx.text_system().add_fonts(fonts) {
        tracing::error!("failed to load bundled fonts: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_fonts_icons_and_logo() {
        for path in [
            "fonts/ibm-plex/IBMPlexSans-Regular.ttf",
            "fonts/ibm-plex/IBMPlexMono-Regular.ttf",
            "icons/lucide/box.svg",
            "logo/png/app-icon-256.png",
        ] {
            assert!(Assets.load(path).unwrap().is_some(), "{path} missing");
        }
    }
}
