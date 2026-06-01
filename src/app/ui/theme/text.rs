use bevy_egui::egui::{self, Color32, FontFamily, FontId, Margin, RichText, TextEdit};

use super::{TITLE_FONT, muted_text};

pub(in crate::app::ui) fn text_input(value: &mut String) -> TextEdit<'_> {
    TextEdit::singleline(value)
        .vertical_align(egui::Align::Center)
        .margin(Margin::symmetric(10, 5))
}

pub(in crate::app::ui) fn title(text_value: &str, size: f32) -> RichText {
    RichText::new(text_value)
        .font(FontId::new(size, FontFamily::Name(TITLE_FONT.into())))
        .color(Color32::WHITE)
}

pub(in crate::app::ui) fn section(text_value: &str) -> RichText {
    RichText::new(text_value)
        .size(21.0)
        .strong()
        .color(Color32::WHITE)
}

pub(in crate::app::ui) fn muted(text_value: impl Into<String>) -> RichText {
    RichText::new(text_value.into()).color(muted_text())
}

pub(in crate::app::ui) fn field_label(text_value: &str) -> RichText {
    RichText::new(text_value)
        .size(12.0)
        .strong()
        .color(Color32::from_rgb(172, 190, 208))
}

pub(in crate::app::ui) fn status_text(text_value: &str) -> RichText {
    RichText::new(text_value)
        .size(13.0)
        .color(Color32::from_rgb(172, 207, 255))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_builders_accept_expected_labels() {
        let mut value = "value".to_owned();
        let _ = text_input(&mut value);
        let _ = title("Ashwend", 78.0);
        let _ = section("Worlds");
        let _ = muted("Muted");
        let _ = field_label("Name");
        let _ = status_text("Ready");

        assert_eq!(value, "value");
    }
}
