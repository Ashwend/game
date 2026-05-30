//! Tabbed options panel. The shell (header + tab bar + body wrapper) lives
//! here; each tab's controls live in a focused submodule. Adding a new tab
//! means adding a new variant in [`OptionsTab`] (see `state/options_ui.rs`),
//! a new branch in [`options_body_contents`], and a new submodule.

mod audio_tab;
mod controls_tab;
mod display_tab;
mod general_tab;
mod graphics_tab;
mod keybindings_tab;
mod voice_tab;
mod widgets;

use bevy::input::ButtonInput;
use bevy::prelude::KeyCode;
use bevy::window::Monitor;
use bevy_egui::egui;

use crate::app::state::{ClientSettings, MenuState, OptionsTab, OptionsUiState, Screen};

use super::theme::{self, BOUNDED_PANEL_VERTICAL_PADDING, ButtonKind, COMPACT_ROW_HEIGHT};

const OPTIONS_PANEL_WIDTH: f32 = 760.0;
const OPTIONS_HEADER_HEIGHT: f32 = COMPACT_ROW_HEIGHT;
const OPTIONS_HEADER_GAP: f32 = 12.0;
const OPTIONS_SCROLL_PADDING_Y: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::ui) enum OptionsBackTarget {
    MainMenu,
    PauseMenu,
}

pub(in crate::app::ui) fn options_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    settings: &mut ClientSettings,
    options_ui_state: &mut OptionsUiState,
    physical_keys: &ButtonInput<KeyCode>,
    primary_monitor: Option<&Monitor>,
    back_target: OptionsBackTarget,
) {
    theme::screen_scrim(ctx, "options_scrim", 145);
    handle_options_escape(ctx, menu, options_ui_state, back_target);

    theme::bounded_panel(
        ctx,
        "options_panel",
        OPTIONS_PANEL_WIDTH,
        BOUNDED_PANEL_VERTICAL_PADDING,
        BOUNDED_PANEL_VERTICAL_PADDING,
        |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(OPTIONS_HEADER_HEIGHT);
                ui.label(theme::section("Options"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                        close_options(menu, options_ui_state, back_target);
                    }
                    if theme::compact_button(ui, "Reset", ButtonKind::Secondary, 78.0).clicked() {
                        *settings = ClientSettings::default();
                    }
                });
            });

            ui.add_space(OPTIONS_HEADER_GAP);
            tab_bar(ui, options_ui_state);
            ui.add_space(OPTIONS_HEADER_GAP);

            let body_height = ui.available_height();
            egui::ScrollArea::vertical()
                .id_salt("options_scroll")
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    options_body_contents(
                        ui,
                        settings,
                        options_ui_state,
                        physical_keys,
                        primary_monitor,
                    );
                });
        },
    );
}

fn options_body_contents(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    options_ui_state: &mut OptionsUiState,
    physical_keys: &ButtonInput<KeyCode>,
    primary_monitor: Option<&Monitor>,
) {
    ui.set_width(ui.available_width());
    ui.add_space(OPTIONS_SCROLL_PADDING_Y);
    match options_ui_state.tab {
        OptionsTab::General => general_tab::render(ui, settings),
        OptionsTab::Display => display_tab::render(ui, settings, primary_monitor),
        OptionsTab::Graphics => graphics_tab::render(ui, settings),
        OptionsTab::Audio => audio_tab::render(ui, settings),
        OptionsTab::Voice => voice_tab::render(ui, settings),
        OptionsTab::Controls => controls_tab::render(ui, settings),
        OptionsTab::Keybindings => {
            keybindings_tab::render(ui, settings, options_ui_state, physical_keys)
        }
    }
    ui.add_space(OPTIONS_SCROLL_PADDING_Y);
}

fn tab_bar(ui: &mut egui::Ui, options_ui_state: &mut OptionsUiState) {
    let frame = egui::Frame::NONE
        .fill(theme::input_fill())
        .stroke(egui::Stroke::new(1.0, theme::panel_stroke()))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(6, 5));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let total = OptionsTab::ALL.len() as f32;
            let spacing = ui.spacing().item_spacing.x;
            let width = ((ui.available_width() - spacing * (total - 1.0)) / total).max(72.0);
            for tab in OptionsTab::ALL {
                let active = options_ui_state.tab == tab;
                let kind = if active {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                };
                if theme::compact_button(ui, tab.label(), kind, width).clicked() {
                    options_ui_state.tab = tab;
                    options_ui_state.pending_rebind = None;
                }
            }
        });
    });
}

fn handle_options_escape(
    ctx: &egui::Context,
    menu: &mut MenuState,
    options_ui_state: &mut OptionsUiState,
    back_target: OptionsBackTarget,
) {
    if !ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        return;
    }
    if options_ui_state.pending_rebind.is_some() {
        options_ui_state.pending_rebind = None;
        return;
    }
    close_options(menu, options_ui_state, back_target);
}

fn close_options(
    menu: &mut MenuState,
    options_ui_state: &mut OptionsUiState,
    back_target: OptionsBackTarget,
) {
    options_ui_state.pending_rebind = None;
    match back_target {
        OptionsBackTarget::MainMenu => {
            menu.screen = Screen::MainMenu;
            menu.pause_open = false;
            menu.pause_options_open = false;
        }
        OptionsBackTarget::PauseMenu => {
            menu.screen = Screen::InGame;
            menu.pause_open = true;
            menu.pause_options_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_input() -> egui::RawInput {
        raw_input_with_size(960.0, 720.0)
    }

    fn raw_input_with_size(width: f32, height: f32) -> egui::RawInput {
        raw_input_with_size_and_events(width, height, Vec::new())
    }

    fn raw_input_with_events(events: Vec<egui::Event>) -> egui::RawInput {
        raw_input_with_size_and_events(960.0, 720.0, events)
    }

    fn raw_input_with_size_and_events(
        width: f32,
        height: f32,
        events: Vec<egui::Event>,
    ) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, height),
            )),
            events,
            ..Default::default()
        }
    }

    fn key_press(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn options_screen_renders_with_fallback_resolutions() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState::default();
        let keys = ButtonInput::default();

        let output = ctx.run(raw_input(), |ctx| {
            options_ui(
                ctx,
                &mut menu,
                &mut settings,
                &mut state,
                &keys,
                None,
                OptionsBackTarget::MainMenu,
            );
        });

        assert!(!output.shapes.is_empty());
        assert_eq!(menu.screen, Screen::Options);
    }

    #[test]
    fn options_screen_renders_on_short_and_tall_viewports() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState::default();
        let keys = ButtonInput::default();

        let short = ctx.run(raw_input_with_size(560.0, 320.0), |ctx| {
            options_ui(
                ctx,
                &mut menu,
                &mut settings,
                &mut state,
                &keys,
                None,
                OptionsBackTarget::MainMenu,
            );
        });
        assert!(!short.shapes.is_empty());

        let ctx = egui::Context::default();
        let tall = ctx.run(raw_input_with_size(960.0, 1440.0), |ctx| {
            options_ui(
                ctx,
                &mut menu,
                &mut settings,
                &mut state,
                &keys,
                None,
                OptionsBackTarget::MainMenu,
            );
        });
        assert!(!tall.shapes.is_empty());
    }

    #[test]
    fn escape_returns_to_main_menu_from_main_options() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState::default();
        let keys = ButtonInput::default();

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                options_ui(
                    ctx,
                    &mut menu,
                    &mut settings,
                    &mut state,
                    &keys,
                    None,
                    OptionsBackTarget::MainMenu,
                );
            },
        );

        assert_eq!(menu.screen, Screen::MainMenu);
        assert!(!menu.pause_open);
        assert!(!menu.pause_options_open);
    }

    #[test]
    fn escape_returns_to_pause_menu_from_ingame_options() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::InGame,
            pause_open: true,
            pause_options_open: true,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState::default();
        let keys = ButtonInput::default();

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                options_ui(
                    ctx,
                    &mut menu,
                    &mut settings,
                    &mut state,
                    &keys,
                    None,
                    OptionsBackTarget::PauseMenu,
                );
            },
        );

        assert_eq!(menu.screen, Screen::InGame);
        assert!(menu.pause_open);
        assert!(!menu.pause_options_open);
    }

    #[test]
    fn escape_cancels_pending_rebind_without_leaving_options() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();
        let mut state = OptionsUiState {
            tab: OptionsTab::Keybindings,
            pending_rebind: Some(crate::app::state::PendingRebind {
                action: crate::app::state::KeyAction::Jump,
                slot: crate::app::state::KeyBindingSlot::Primary,
            }),
        };
        let keys = ButtonInput::default();

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                options_ui(
                    ctx,
                    &mut menu,
                    &mut settings,
                    &mut state,
                    &keys,
                    None,
                    OptionsBackTarget::MainMenu,
                );
            },
        );

        assert_eq!(menu.screen, Screen::Options);
        assert!(state.pending_rebind.is_none());
    }
}
