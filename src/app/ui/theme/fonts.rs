use std::sync::Arc;

use bevy::log::warn;
use bevy_egui::egui::{self, FontData, FontDefinitions, FontFamily};

use crate::app::embedded_assets::embedded_bytes;

/// Named egui font family the hero title renders with. Reference it via
/// `FontFamily::Name(TITLE_FONT.into())` (see [`super::title`]).
pub(in crate::app::ui) const TITLE_FONT: &str = "cinzel";

/// `assets/`-relative path of the embedded title typeface. Cinzel is a
/// Roman-inscription serif (SIL OFL, see `assets/fonts/OFL.txt`) whose
/// capital letterforms are built for all-caps display, hence the
/// uppercase "ASHWEND" on the main menu.
const TITLE_FONT_PATH: &str = "fonts/Cinzel-Bold.ttf";

/// Registers the title typeface on the egui context, keeping the default
/// proportional fonts as a glyph fallback. Call once after the primary
/// context exists; `ctx.set_fonts` rebuilds the font atlas, so it must not
/// run every frame.
pub(in crate::app::ui) fn install_title_font(ctx: &egui::Context) {
    let Some(bytes) = embedded_bytes(TITLE_FONT_PATH) else {
        warn!("title font `{TITLE_FONT_PATH}` not embedded; using egui default");
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        TITLE_FONT.to_owned(),
        Arc::new(FontData::from_static(bytes)),
    );

    // Cinzel first, then whatever the default proportional chain offers, so
    // any glyph Cinzel lacks (it ships latin only) still renders.
    let mut chain = vec![TITLE_FONT.to_owned()];
    if let Some(proportional) = fonts.families.get(&FontFamily::Proportional) {
        chain.extend(proportional.iter().cloned());
    }
    fonts
        .families
        .insert(FontFamily::Name(TITLE_FONT.into()), chain);

    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_font_is_embedded() {
        let bytes = embedded_bytes(TITLE_FONT_PATH).expect("title font must be embedded");
        // TrueType outlines begin with the 0x00010000 sfnt version tag.
        assert_eq!(&bytes[0..4], &[0x00, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn install_registers_named_family() {
        let ctx = egui::Context::default();
        install_title_font(&ctx);
        // `Context::fonts` panics until the first `run`, and `set_fonts`
        // takes effect at the next `begin_pass`, so probe inside a pass.
        let mut registered = false;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            registered = ui
                .ctx()
                .fonts(|f| f.families().contains(&FontFamily::Name(TITLE_FONT.into())));
        });
        assert!(registered, "title font family was not registered");
    }
}
