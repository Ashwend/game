use bevy_egui::egui::Color32;

pub(in crate::app::ui) fn text() -> Color32 {
    Color32::from_rgb(224, 231, 238)
}

pub(in crate::app::ui) fn muted_text() -> Color32 {
    Color32::from_rgb(146, 158, 171)
}

/// Inline error / validation text (failed logins, invalid world names, connect
/// failures, update failures). One warm red so every error state across the UI
/// reads as the same thing, instead of the several near-identical hand-rolled
/// reds that drifted apart per screen.
pub(in crate::app::ui) fn error_text() -> Color32 {
    Color32::from_rgb(236, 124, 112)
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

/// Backdrop tint drawn behind every full-screen modal (crafting,
/// furnace, inventory, loot bag, pause). Near-black with high alpha
/// so the modal pops while leaving a hint of the world visible
/// underneath.
pub(in crate::app::ui) fn backdrop_color() -> Color32 {
    Color32::from_rgba_unmultiplied(1, 3, 7, 190)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_values_are_stable() {
        assert_eq!(text(), Color32::from_rgb(224, 231, 238));
        assert_eq!(muted_text(), Color32::from_rgb(146, 158, 171));
        assert_eq!(error_text(), Color32::from_rgb(236, 124, 112));
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
