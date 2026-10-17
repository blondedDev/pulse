use gpui::AssetSource;
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "assets"]
#[include = "icons/**/*"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow::anyhow!("asset not found: {path}"))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<gpui::SharedString>> {
        Ok(Self::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| p.into())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mistyped asset path still compiles; it only shows up as a blank button
    /// at runtime. These tests turn that into a build failure.
    #[test]
    fn the_folder_tab_icon_is_embedded() {
        assert!(Assets::get("icons/s1-folder.svg").is_some());
    }

    #[test]
    fn every_embedded_icon_loads_as_an_svg() {
        for path in Assets::iter() {
            let asset = Assets::get(&path).expect("iter yielded an unloadable path");
            let body = String::from_utf8_lossy(&asset.data);
            assert!(body.contains("<svg"), "{path} is not an SVG document");
        }
    }
}
