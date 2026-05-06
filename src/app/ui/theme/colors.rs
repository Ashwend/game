use bevy_egui::egui::Color32;

pub(in crate::app::ui) fn text() -> Color32 {
    Color32::from_rgb(224, 231, 238)
}

pub(in crate::app::ui) fn muted_text() -> Color32 {
    Color32::from_rgb(146, 158, 171)
}

pub(in crate::app::ui) fn accent() -> Color32 {
    Color32::from_rgb(92, 162, 255)
}

pub(in crate::app::ui) fn accent_dark() -> Color32 {
    Color32::from_rgb(31, 82, 141)
}

pub(in crate::app::ui) fn panel_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(12, 17, 23, 232)
}

pub(in crate::app::ui) fn panel_stroke() -> Color32 {
    Color32::from_rgba_unmultiplied(112, 132, 154, 106)
}

pub(in crate::app::ui) fn input_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(6, 9, 13, 232)
}

pub(in crate::app::ui) fn button_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(38, 45, 54, 232)
}

pub(in crate::app::ui) fn button_hover_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(53, 63, 75, 242)
}

pub(in crate::app::ui) fn button_stroke() -> Color32 {
    Color32::from_rgba_unmultiplied(115, 132, 151, 112)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_values_are_stable() {
        assert_eq!(text(), Color32::from_rgb(224, 231, 238));
        assert_eq!(muted_text(), Color32::from_rgb(146, 158, 171));
        assert_eq!(accent(), Color32::from_rgb(92, 162, 255));
        assert_eq!(accent_dark(), Color32::from_rgb(31, 82, 141));
        assert_eq!(
            panel_fill(),
            Color32::from_rgba_unmultiplied(12, 17, 23, 232)
        );
        assert_eq!(
            panel_stroke(),
            Color32::from_rgba_unmultiplied(112, 132, 154, 106)
        );
        assert_eq!(input_fill(), Color32::from_rgba_unmultiplied(6, 9, 13, 232));
        assert_eq!(
            button_fill(),
            Color32::from_rgba_unmultiplied(38, 45, 54, 232)
        );
        assert_eq!(
            button_hover_fill(),
            Color32::from_rgba_unmultiplied(53, 63, 75, 242)
        );
        assert_eq!(
            button_stroke(),
            Color32::from_rgba_unmultiplied(115, 132, 151, 112)
        );
    }
}
