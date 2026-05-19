use bevy::window::Monitor;
use bevy_egui::egui;

use crate::app::state::{ClientSettings, DisplayMode, MenuState, Screen, display_resolutions};

use super::theme::{self, ButtonKind};

const OPTIONS_PANEL_WIDTH: f32 = 720.0;
const OPTIONS_SCREEN_MARGIN_X: f32 = 56.0;
const OPTIONS_SCREEN_MARGIN_Y: f32 = 24.0;
const OPTIONS_PANEL_INNER_X: f32 = 48.0;
const OPTIONS_PANEL_INNER_Y: f32 = 44.0;
const OPTIONS_HEADER_HEIGHT: f32 = 34.0;
const OPTIONS_HEADER_GAP: f32 = 12.0;
const OPTIONS_SCROLL_PADDING_Y: f32 = 8.0;
const OPTIONS_MIN_BODY_HEIGHT: f32 = 96.0;
const OPTIONS_FULL_CONTENT_HEIGHT: f32 = 520.0;
const SETTING_LABEL_WIDTH: f32 = 190.0;
const SETTING_CONTROL_WIDTH: f32 = 260.0;
const SETTING_ROW_HEIGHT: f32 = 36.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OptionsBackTarget {
    MainMenu,
    PauseMenu,
}

pub(super) fn options_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
    back_target: OptionsBackTarget,
) {
    theme::screen_scrim(ctx, "options_scrim", 145);
    handle_options_escape(ctx, menu, back_target);
    options_panel(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_height(OPTIONS_HEADER_HEIGHT);
            ui.label(theme::section("Options"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                    close_options(menu, back_target);
                }
                if theme::compact_button(ui, "Reset", ButtonKind::Secondary, 78.0).clicked() {
                    *settings = ClientSettings::default();
                }
            });
        });

        ui.add_space(OPTIONS_HEADER_GAP);
        let body_height = options_body_max_height(ctx.content_rect().height());
        if options_body_needs_scroll(body_height) {
            egui::ScrollArea::vertical()
                .id_salt("options_scroll")
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    options_body_contents(ui, settings, primary_monitor);
                });
        } else {
            options_body_contents(ui, settings, primary_monitor);
        }
    });
}

fn options_panel(ctx: &egui::Context, add_contents: impl FnOnce(&mut egui::Ui)) {
    let screen = ctx.content_rect();
    let width = OPTIONS_PANEL_WIDTH.min((screen.width() - OPTIONS_SCREEN_MARGIN_X).max(300.0));
    egui::Area::new("options_panel".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_width(width);
            theme::panel_frame().show(ui, |ui| {
                ui.set_width(width - OPTIONS_PANEL_INNER_X);
                add_contents(ui);
            });
        });
}

fn options_body_max_height(screen_height: f32) -> f32 {
    (screen_height
        - OPTIONS_SCREEN_MARGIN_Y * 2.0
        - OPTIONS_PANEL_INNER_Y
        - OPTIONS_HEADER_HEIGHT
        - OPTIONS_HEADER_GAP)
        .max(OPTIONS_MIN_BODY_HEIGHT)
}

fn options_body_needs_scroll(body_height: f32) -> bool {
    body_height < OPTIONS_FULL_CONTENT_HEIGHT
}

fn options_body_contents(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
) {
    ui.set_width(ui.available_width());
    ui.add_space(OPTIONS_SCROLL_PADDING_Y);
    options_sections(ui, settings, primary_monitor);
    ui.add_space(OPTIONS_SCROLL_PADDING_Y);
}

fn options_sections(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
) {
    theme::inset_frame().show(ui, |ui| {
        ui.label(options_section_label("Display"));
        ui.add_space(6.0);
        display_mode_row(ui, settings);
        resolution_row(ui, settings, primary_monitor);
        setting_row(ui, "VSync", |ui| {
            checkbox_with_click_sound(ui, &mut settings.display.vsync, "Enabled");
        });
    });

    ui.add_space(12.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(options_section_label("Audio"));
        ui.add_space(6.0);
        percent_slider_row(ui, "Music Volume", &mut settings.audio.music_volume);
        percent_slider_row(ui, "Effects Volume", &mut settings.audio.sfx_volume);
    });

    ui.add_space(12.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(options_section_label("Input"));
        ui.add_space(6.0);
        sensitivity_row(ui, settings);
        setting_row(ui, "Invert Mouse Y", |ui| {
            checkbox_with_click_sound(ui, &mut settings.input.invert_mouse_y, "Enabled");
        });
    });

    ui.add_space(12.0);
    theme::inset_frame().show(ui, |ui| {
        ui.label(options_section_label("HUD"));
        ui.add_space(6.0);
        setting_row(ui, "FPS Counter", |ui| {
            checkbox_with_click_sound(ui, &mut settings.hud.show_fps, "Enabled");
        });
    });
}

fn options_section_label(label: &str) -> egui::RichText {
    egui::RichText::new(label)
        .size(14.0)
        .strong()
        .color(egui::Color32::from_rgb(196, 216, 236))
}

fn handle_options_escape(
    ctx: &egui::Context,
    menu: &mut MenuState,
    back_target: OptionsBackTarget,
) {
    if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
        close_options(menu, back_target);
    }
}

fn close_options(menu: &mut MenuState, back_target: OptionsBackTarget) {
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

fn display_mode_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    setting_row(ui, "Display Mode", |ui| {
        let response = egui::ComboBox::from_id_salt("options_display_mode")
            .selected_text(settings.display.mode.label())
            .width(230.0)
            .show_ui(ui, |ui| {
                for mode in DisplayMode::ALL {
                    let response =
                        ui.selectable_value(&mut settings.display.mode, mode, mode.label());
                    theme::record_click_sound(ui, &response);
                }
            })
            .response;
        theme::record_click_sound(ui, &response);
    });
}

fn resolution_row(
    ui: &mut egui::Ui,
    settings: &mut ClientSettings,
    primary_monitor: Option<&Monitor>,
) {
    let mut resolutions = display_resolutions(primary_monitor, settings.display.mode);
    if settings.display.mode != DisplayMode::Fullscreen
        && !resolutions.contains(&settings.display.resolution)
    {
        resolutions.push(settings.display.resolution);
    }
    resolutions.sort_by_key(|resolution| {
        (
            u64::from(resolution.width) * u64::from(resolution.height),
            resolution.width,
            resolution.height,
        )
    });

    if settings.display.mode == DisplayMode::Fullscreen
        && !resolutions.contains(&settings.display.resolution)
        && let Some(resolution) = resolutions.last().copied()
    {
        settings.display.resolution = resolution;
    }

    let enabled = settings.display.mode != DisplayMode::BorderlessFullscreen;
    let selected_text = if enabled {
        settings.display.resolution.label()
    } else {
        "Native Display".to_owned()
    };

    setting_row(ui, "Resolution", |ui| {
        ui.add_enabled_ui(enabled, |ui| {
            let response = egui::ComboBox::from_id_salt("options_resolution")
                .selected_text(selected_text)
                .width(230.0)
                .show_ui(ui, |ui| {
                    for resolution in resolutions {
                        let response = ui.selectable_value(
                            &mut settings.display.resolution,
                            resolution,
                            resolution.label(),
                        );
                        theme::record_click_sound(ui, &response);
                    }
                })
                .response;
            theme::record_click_sound(ui, &response);
        });
    });
}

fn sensitivity_row(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    let mut value = settings.input.mouse_sensitivity * 100.0;
    setting_row(ui, "Mouse Sensitivity", |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(&mut value, 25.0..=300.0)
                    .suffix("%")
                    .show_value(true),
            )
            .changed()
        {
            settings.input.mouse_sensitivity = (value / 100.0).clamp(0.25, 3.0);
        }
    });
}

fn percent_slider_row(ui: &mut egui::Ui, label: &str, value: &mut f32) {
    let mut percent = *value * 100.0;
    setting_row(ui, label, |ui| {
        let control_width = ui.available_width();
        if ui
            .add_sized(
                [control_width, SETTING_ROW_HEIGHT],
                egui::Slider::new(&mut percent, 0.0..=100.0)
                    .suffix("%")
                    .show_value(true),
            )
            .changed()
        {
            *value = (percent / 100.0).clamp(0.0, 1.0);
        }
    });
}

fn checkbox_with_click_sound(ui: &mut egui::Ui, value: &mut bool, label: &str) {
    let response = ui
        .checkbox(value, label)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    theme::record_click_sound(ui, &response);
}

fn setting_row(ui: &mut egui::Ui, label: &str, add_control: impl FnOnce(&mut egui::Ui)) {
    let row_width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(row_width, SETTING_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(SETTING_LABEL_WIDTH, SETTING_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(theme::muted(label));
                },
            );

            let spacer = (ui.available_width() - SETTING_CONTROL_WIDTH).max(0.0);
            if spacer > 0.0 {
                ui.add_space(spacer);
            }
            ui.allocate_ui_with_layout(
                egui::vec2(
                    SETTING_CONTROL_WIDTH.min(ui.available_width()),
                    SETTING_ROW_HEIGHT,
                ),
                egui::Layout::right_to_left(egui::Align::Center),
                add_control,
            );
        },
    );
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

        let output = ctx.run(raw_input(), |ctx| {
            options_ui(
                ctx,
                &mut menu,
                &mut settings,
                None,
                OptionsBackTarget::MainMenu,
            );
        });

        assert!(!output.shapes.is_empty());
        assert_eq!(menu.screen, Screen::Options);
    }

    #[test]
    fn options_screen_caps_body_height_on_short_viewports() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();

        let output = ctx.run(raw_input_with_size(560.0, 320.0), |ctx| {
            options_ui(
                ctx,
                &mut menu,
                &mut settings,
                None,
                OptionsBackTarget::MainMenu,
            );
        });

        assert!(!output.shapes.is_empty());
        assert_eq!(options_body_max_height(320.0), 182.0);
        assert!(options_body_needs_scroll(options_body_max_height(320.0)));
        assert!(!options_body_needs_scroll(options_body_max_height(1440.0)));
    }

    #[test]
    fn escape_returns_to_main_menu_from_main_options() {
        let ctx = egui::Context::default();
        let mut menu = MenuState {
            screen: Screen::Options,
            ..Default::default()
        };
        let mut settings = ClientSettings::default();

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                options_ui(
                    ctx,
                    &mut menu,
                    &mut settings,
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

        let _ = ctx.run(
            raw_input_with_events(vec![key_press(egui::Key::Escape)]),
            |ctx| {
                options_ui(
                    ctx,
                    &mut menu,
                    &mut settings,
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
    fn borderless_fullscreen_disables_resolution_selection() {
        let ctx = egui::Context::default();
        let mut settings = ClientSettings::default();
        settings.display.mode = DisplayMode::BorderlessFullscreen;

        let output = ctx.run(raw_input(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                resolution_row(ui, &mut settings, None);
            });
        });

        assert!(!output.shapes.is_empty());
        assert_eq!(settings.display.mode, DisplayMode::BorderlessFullscreen);
    }
}
