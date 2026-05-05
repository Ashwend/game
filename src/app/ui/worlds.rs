use anyhow::Context;
use bevy_egui::egui;
use uuid::Uuid;

use crate::{
    app::state::{ClientRuntime, MenuState, SaveStore, Screen, SteamUser},
    net::ClientSession,
};

use super::theme::{self, ButtonKind};

pub(super) fn worlds_ui(
    ctx: &egui::Context,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
) {
    theme::screen_scrim(ctx, "worlds_scrim", 145);
    theme::anchored_panel(
        ctx,
        "worlds_panel",
        920.0,
        egui::Align2::CENTER_CENTER,
        [0.0, -8.0],
        |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::section("Singleplayer Worlds"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::compact_button(ui, "Back", ButtonKind::Secondary, 78.0).clicked() {
                        menu.screen = Screen::MainMenu;
                    }
                    if theme::compact_button(ui, "Refresh", ButtonKind::Secondary, 88.0).clicked() {
                        refresh_worlds(menu, store);
                    }
                });
            });

            ui.add_space(16.0);
            theme::inset_frame().show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(theme::field_label("New World"));
                    ui.add_sized(
                        [360.0, 34.0],
                        egui::TextEdit::singleline(&mut menu.new_world_name),
                    );
                    if theme::compact_button(ui, "Create", ButtonKind::Primary, 92.0).clicked() {
                        match store
                            .0
                            .create_world(&menu.new_world_name, Some(user.0.steam_id))
                        {
                            Ok(_) => {
                                menu.new_world_name = "New World".to_owned();
                                refresh_worlds(menu, store);
                            }
                            Err(error) => menu.status = Some(format!("create failed: {error}")),
                        }
                    }
                });
            });

            ui.add_space(14.0);
            draw_world_headers(ui);

            if menu.worlds.is_empty() {
                theme::inset_frame().show(ui, |ui| {
                    ui.set_min_height(120.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(38.0);
                        ui.label(theme::muted("No worlds yet."));
                    });
                });
            } else {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .max_height((ctx.content_rect().height() - 260.0).max(180.0))
                    .show(ui, |ui| {
                        let worlds = menu.worlds.clone();
                        for world in worlds {
                            draw_world_row(ui, menu, runtime, store, user, world);
                            ui.add_space(8.0);
                        }
                    });
            }

            if let Some(status) = &menu.status {
                ui.add_space(10.0);
                ui.label(theme::status_text(status));
            }
        },
    );
}

fn draw_world_headers(ui: &mut egui::Ui) {
    let columns = WorldColumns::for_width(ui.available_width());
    ui.horizontal(|ui| {
        ui.add_sized(
            [columns.name, 18.0],
            egui::Label::new(theme::field_label("World")),
        );
        ui.add_sized(
            [columns.seed, 18.0],
            egui::Label::new(theme::field_label("Seed")),
        );
        ui.add_sized(
            [columns.admins, 18.0],
            egui::Label::new(theme::field_label("Admins")),
        );
        ui.add_sized(
            [columns.actions, 18.0],
            egui::Label::new(theme::field_label("Actions")),
        );
    });
    ui.add_space(6.0);
}

fn draw_world_row(
    ui: &mut egui::Ui,
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    world: crate::save::WorldSummary,
) {
    theme::inset_frame().show(ui, |ui| {
        let columns = WorldColumns::for_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add_sized(
                [columns.name, 24.0],
                egui::Label::new(egui::RichText::new(&world.name).strong()).truncate(),
            );
            ui.add_sized(
                [columns.seed, 24.0],
                egui::Label::new(
                    egui::RichText::new(world.seed.to_string())
                        .monospace()
                        .color(theme::muted_text()),
                )
                .truncate(),
            );
            ui.add_sized(
                [columns.admins, 24.0],
                egui::Label::new(world.admin_count.to_string()),
            );

            if theme::compact_button(ui, "Start", ButtonKind::Primary, 78.0).clicked() {
                start_singleplayer(menu, runtime, store, user, world.id);
            }
            if theme::compact_button(ui, "Delete", ButtonKind::Danger, 82.0).clicked() {
                match store.0.delete_world(world.id) {
                    Ok(()) => refresh_worlds(menu, store),
                    Err(error) => menu.status = Some(format!("delete failed: {error}")),
                }
            }
        });
    });
}

#[derive(Debug, Clone, Copy)]
struct WorldColumns {
    name: f32,
    seed: f32,
    admins: f32,
    actions: f32,
}

impl WorldColumns {
    fn for_width(width: f32) -> Self {
        let actions = 172.0;
        let admins = 72.0;
        let seed = (width * 0.25).clamp(145.0, 205.0);
        let spacing_allowance = 48.0;
        let name = (width - actions - admins - seed - spacing_allowance).max(150.0);

        Self {
            name,
            seed,
            admins,
            actions,
        }
    }
}

pub(super) fn refresh_worlds(menu: &mut MenuState, store: &SaveStore) {
    match store.0.list_worlds() {
        Ok(worlds) => {
            menu.worlds = worlds;
            menu.status = None;
        }
        Err(error) => {
            menu.worlds.clear();
            menu.status = Some(format!("world list failed: {error}"));
        }
    }
}

fn start_singleplayer(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    store: &SaveStore,
    user: &SteamUser,
    world_id: Uuid,
) {
    let result = store
        .0
        .load_world(world_id)
        .context("could not load selected world")
        .and_then(|save| ClientSession::start_singleplayer(save, &user.0));

    match result {
        Ok(session) => {
            runtime.start_session(session, Some(world_id));
            menu.screen = Screen::InGame;
            menu.pause_open = false;
            menu.status = None;
        }
        Err(error) => menu.status = Some(format!("start failed: {error}")),
    }
}
