mod buttons;
mod colors;
mod frames;
mod text;
mod tooltip;

pub(super) use buttons::{
    ButtonKind, ButtonSound, compact_button, compact_button_in_rect, disabled_game_button,
    game_button, record_click_sound, take_button_sounds,
};
pub(super) use colors::{
    accent, accent_dark, button_fill, button_hover_fill, button_stroke, input_fill, muted_text,
    panel_fill, panel_stroke, text,
};
pub(super) use frames::{
    anchored_panel, apply_game_style, backdrop_cover, inset_frame, panel_frame, screen_scrim,
};
pub(super) use text::{field_label, muted, section, status_text, text_input, title};
pub(super) use tooltip::wow_tooltip;

pub(super) const MENU_WIDTH: f32 = 360.0;
