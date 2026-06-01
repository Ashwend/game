mod buttons;
mod colors;
mod fonts;
mod frames;
mod spacing;
mod text;
mod tooltip;

pub(super) use buttons::{
    ButtonKind, ButtonSound, ButtonState, button_paint_rest, compact_button,
    compact_button_in_rect, compact_button_in_rect_with_state, compact_button_with_state,
    game_button, record_click_sound, take_button_sounds,
};
pub(super) use colors::{
    accent, accent_dark, backdrop_color, button_fill, button_hover_fill, button_stroke, input_fill,
    muted_text, panel_fill, panel_stroke, text,
};
pub(super) use fonts::{TITLE_FONT, install_title_font};
pub(super) use frames::{
    apply_game_style, backdrop_cover, bounded_panel, inset_frame, panel_frame, screen_scrim,
};
pub(super) use spacing::{BOUNDED_PANEL_VERTICAL_PADDING, COMPACT_ROW_HEIGHT};
pub(super) use text::{field_label, muted, section, status_text, text_input, title};
pub(super) use tooltip::{anchored_wow_tooltip, wow_tooltip};

pub(super) const MENU_WIDTH: f32 = 360.0;
pub(super) const MENU_BUTTON_WIDTH: f32 = 260.0;
