use std::hash::Hash;

use bevy_egui::egui::{
    self, Align2, Button, Color32, CursorIcon, FontFamily, FontId, Id, RichText, Sense, Stroke,
    Vec2,
};

use super::{accent, accent_dark, button_fill, button_hover_fill, button_stroke, muted_text, text};

#[derive(Debug, Clone, Copy)]
pub(in crate::app::ui) enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

#[derive(Debug, Clone, Copy)]
enum ButtonDensity {
    Menu,
    Compact,
}

#[derive(Debug, Clone, Copy)]
enum ButtonInteraction {
    Rest,
    Hovered,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::ui) enum ButtonSound {
    Hover,
    Click,
}

#[derive(Debug, Clone, Copy)]
struct ButtonSpec {
    height: f32,
    font_size: f32,
    strong: bool,
}

impl ButtonDensity {
    fn spec(self) -> ButtonSpec {
        match self {
            Self::Menu => ButtonSpec {
                height: 46.0,
                font_size: 14.0,
                strong: true,
            },
            Self::Compact => ButtonSpec {
                height: 34.0,
                font_size: 13.0,
                strong: false,
            },
        }
    }
}

pub(in crate::app::ui) fn game_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: ButtonKind,
    width: f32,
) -> egui::Response {
    sized_button(ui, label, kind, ButtonDensity::Menu, width)
}

pub(in crate::app::ui) fn disabled_game_button(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
) -> egui::Response {
    let height = ButtonDensity::Menu.spec().height;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    ui.painter().rect(
        rect,
        4,
        Color32::from_rgba_unmultiplied(28, 32, 38, 210),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(92, 102, 116, 72)),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(14.0, FontFamily::Proportional),
        muted_text(),
    );

    response
}

pub(in crate::app::ui) fn compact_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: ButtonKind,
    width: f32,
) -> egui::Response {
    sized_button(ui, label, kind, ButtonDensity::Compact, width)
}

pub(in crate::app::ui) fn compact_button_in_rect(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    rect: egui::Rect,
    label: &str,
    kind: ButtonKind,
) -> egui::Response {
    let response = ui
        .interact(rect, ui.id().with(id_source), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let (fill, stroke, text_color) =
        button_paint(kind, ButtonDensity::Compact, button_interaction(&response));

    ui.painter()
        .rect(rect, 4, fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::new(13.0, FontFamily::Proportional),
        text_color,
    );

    record_button_sounds(ui, &response);
    response
}

pub(in crate::app::ui) fn take_button_sounds(ctx: &egui::Context) -> Vec<ButtonSound> {
    ctx.data_mut(|data| {
        data.remove_temp::<Vec<ButtonSound>>(button_sound_queue_id())
            .unwrap_or_default()
    })
}

pub(in crate::app::ui) fn record_click_sound(ui: &egui::Ui, response: &egui::Response) {
    if response.clicked() {
        queue_button_sound(ui, ButtonSound::Click);
    }
}

fn sized_button(
    ui: &mut egui::Ui,
    label: &str,
    kind: ButtonKind,
    density: ButtonDensity,
    width: f32,
) -> egui::Response {
    let spec = density.spec();
    let (fill, stroke, text_color) = button_paint(kind, density, ButtonInteraction::Rest);
    let mut label = RichText::new(label).size(spec.font_size).color(text_color);
    if spec.strong {
        label = label.strong();
    }

    let response = ui
        .add(
            Button::new(label)
                .min_size(Vec2::new(width, spec.height))
                .fill(fill)
                .stroke(stroke)
                .corner_radius(4),
        )
        .on_hover_cursor(CursorIcon::PointingHand);

    record_button_sounds(ui, &response);
    response
}

fn record_button_sounds(ui: &mut egui::Ui, response: &egui::Response) {
    record_click_sound(ui, response);

    let hovered = response.hovered();
    let hover_state_id = response.id.with("hover_sound");
    let was_hovered = ui.data_mut(|data| {
        let was_hovered = data.get_persisted::<bool>(hover_state_id).unwrap_or(false);
        data.insert_persisted(hover_state_id, hovered);
        was_hovered
    });
    if hover_sound_entered(was_hovered, hovered) {
        queue_button_sound(ui, ButtonSound::Hover);
    }
}

fn queue_button_sound(ui: &egui::Ui, sound: ButtonSound) {
    ui.ctx().data_mut(|data| {
        let id = button_sound_queue_id();
        let mut sounds = data.get_temp::<Vec<ButtonSound>>(id).unwrap_or_default();
        sounds.push(sound);
        data.insert_temp(id, sounds);
    });
}

fn button_sound_queue_id() -> Id {
    Id::new("button_sound_queue")
}

fn hover_sound_entered(was_hovered: bool, hovered: bool) -> bool {
    hovered && !was_hovered
}

fn button_interaction(response: &egui::Response) -> ButtonInteraction {
    if response.is_pointer_button_down_on() {
        ButtonInteraction::Active
    } else if response.hovered() {
        ButtonInteraction::Hovered
    } else {
        ButtonInteraction::Rest
    }
}

fn button_paint(
    kind: ButtonKind,
    density: ButtonDensity,
    interaction: ButtonInteraction,
) -> (Color32, Stroke, Color32) {
    match kind {
        ButtonKind::Primary => {
            let fill = match interaction {
                ButtonInteraction::Rest => accent_dark(),
                ButtonInteraction::Hovered => Color32::from_rgb(37, 101, 174),
                ButtonInteraction::Active => Color32::from_rgb(24, 67, 118),
            };
            (fill, Stroke::new(1.0, accent()), Color32::WHITE)
        }
        ButtonKind::Secondary => {
            let fill = match interaction {
                ButtonInteraction::Rest => button_fill(),
                ButtonInteraction::Hovered => button_hover_fill(),
                ButtonInteraction::Active => Color32::from_rgba_unmultiplied(30, 36, 45, 246),
            };
            (fill, Stroke::new(1.0, button_stroke()), text())
        }
        ButtonKind::Danger => {
            let palette = DangerButtonPalette::for_density(density);
            let fill = match interaction {
                ButtonInteraction::Rest => palette.rest,
                ButtonInteraction::Hovered => palette.hovered,
                ButtonInteraction::Active => palette.active,
            };
            (
                fill,
                Stroke::new(1.0, palette.stroke),
                Color32::from_rgb(255, 224, 224),
            )
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct DangerButtonPalette {
    rest: Color32,
    hovered: Color32,
    active: Color32,
    stroke: Color32,
}

impl DangerButtonPalette {
    fn for_density(density: ButtonDensity) -> Self {
        match density {
            ButtonDensity::Menu => Self {
                rest: Color32::from_rgba_unmultiplied(92, 35, 38, 224),
                hovered: Color32::from_rgba_unmultiplied(92, 35, 38, 224),
                active: Color32::from_rgba_unmultiplied(92, 35, 38, 224),
                stroke: Color32::from_rgb(165, 72, 76),
            },
            ButtonDensity::Compact => Self {
                rest: Color32::from_rgba_unmultiplied(75, 31, 34, 218),
                hovered: Color32::from_rgba_unmultiplied(94, 36, 40, 238),
                active: Color32::from_rgba_unmultiplied(62, 22, 25, 236),
                stroke: Color32::from_rgb(145, 60, 64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_danger_button_uses_interaction_palette() {
        let (rest, _, _) = button_paint(
            ButtonKind::Danger,
            ButtonDensity::Compact,
            ButtonInteraction::Rest,
        );
        let (hovered, _, _) = button_paint(
            ButtonKind::Danger,
            ButtonDensity::Compact,
            ButtonInteraction::Hovered,
        );
        let (active, _, _) = button_paint(
            ButtonKind::Danger,
            ButtonDensity::Compact,
            ButtonInteraction::Active,
        );

        assert_eq!(rest, Color32::from_rgba_unmultiplied(75, 31, 34, 218));
        assert_eq!(hovered, Color32::from_rgba_unmultiplied(94, 36, 40, 238));
        assert_eq!(active, Color32::from_rgba_unmultiplied(62, 22, 25, 236));
    }

    #[test]
    fn menu_buttons_keep_the_larger_button_contract() {
        let spec = ButtonDensity::Menu.spec();
        let (fill, stroke, text_color) = button_paint(
            ButtonKind::Danger,
            ButtonDensity::Menu,
            ButtonInteraction::Rest,
        );

        assert_eq!(spec.height, 46.0);
        assert_eq!(spec.font_size, 14.0);
        assert!(spec.strong);
        assert_eq!(fill, Color32::from_rgba_unmultiplied(92, 35, 38, 224));
        assert_eq!(stroke.color, Color32::from_rgb(165, 72, 76));
        assert_eq!(text_color, Color32::from_rgb(255, 224, 224));
    }

    #[test]
    fn hover_sound_only_triggers_on_hover_entry() {
        assert!(hover_sound_entered(false, true));
        assert!(!hover_sound_entered(true, true));
        assert!(!hover_sound_entered(false, false));
        assert!(!hover_sound_entered(true, false));
    }
}
