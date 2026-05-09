use std::fs;

use bevy_egui::egui;

use crate::{
    app::state::{
        ClientRuntime, CreateWorldDialog, CreateWorldMapKind, EditWorldDialog, MenuState,
        SaveStore, Screen, SteamUser,
    },
    save::WorldStore,
    steam::AuthenticatedUser,
    world::{MapType, ProceduralMapSize},
};

use super::{
    dialogs::{create_world_from_dialog, rename_world_from_dialog},
    session::{refresh_worlds, start_singleplayer},
};

fn raw_input() -> egui::RawInput {
    raw_input_with_events(Vec::new())
}

fn raw_input_with_events(events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1024.0, 768.0),
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

fn temp_store() -> SaveStore {
    SaveStore(WorldStore::new(
        std::env::temp_dir().join(format!("game-worlds-ui-test-{}", uuid::Uuid::new_v4())),
    ))
}

fn steam_user() -> SteamUser {
    SteamUser(AuthenticatedUser {
        steam_id: 42,
        display_name: "Dannie".to_owned(),
        token: "offline:42".to_owned(),
    })
}

#[test]
fn refresh_worlds_handles_success_and_list_errors() {
    let store = temp_store();
    let mut menu = MenuState::default();
    let first = store
        .0
        .create_world("Beta", Some(42))
        .expect("world should create");
    let second = store
        .0
        .create_world("Alpha", Some(42))
        .expect("world should create");

    refresh_worlds(&mut menu, &store);

    assert_eq!(menu.worlds.len(), 2);
    assert!(menu.status.is_none());
    assert!(menu.worlds.iter().any(|world| world.id == first.id));
    assert!(menu.worlds.iter().any(|world| world.id == second.id));

    let bad_root =
        std::env::temp_dir().join(format!("game-worlds-ui-file-{}", uuid::Uuid::new_v4()));
    fs::write(&bad_root, "not a directory").expect("file should write");
    let bad_store = SaveStore(WorldStore::new(&bad_root));
    refresh_worlds(&mut menu, &bad_store);

    assert!(menu.worlds.is_empty());
    assert!(
        menu.status
            .expect("status should exist")
            .contains("world list failed")
    );

    let _ = fs::remove_dir_all(store.0.root());
    let _ = fs::remove_file(bad_root);
}

#[test]
fn start_singleplayer_updates_runtime_or_reports_load_error() {
    let store = temp_store();
    let user = steam_user();
    let save = store
        .0
        .create_world("Local", Some(user.0.steam_id))
        .expect("world should create");
    let mut menu = MenuState::default();
    let mut runtime = ClientRuntime::default();

    start_singleplayer(&mut menu, &mut runtime, &store, &user, save.id);

    assert_eq!(menu.screen, Screen::InGame);
    assert!(!menu.pause_open);
    assert!(!menu.chat_open);
    assert_eq!(runtime.active_world_id, Some(save.id));
    assert!(runtime.session.is_some());

    start_singleplayer(&mut menu, &mut runtime, &store, &user, uuid::Uuid::new_v4());

    assert!(
        menu.status
            .expect("status should exist")
            .contains("start failed")
    );

    let _ = fs::remove_dir_all(store.0.root());
}

#[test]
fn create_world_from_dialog_persists_selected_map() {
    let store = temp_store();
    let user = steam_user();
    let mut menu = MenuState::default();
    let dialog = CreateWorldDialog {
        name: "Generated".to_owned(),
        map_kind: CreateWorldMapKind::Procedural,
        procedural_size: ProceduralMapSize::Small,
        seed: "1234".to_owned(),
        error: None,
        closing: false,
        confirmed: true,
    };

    create_world_from_dialog(dialog, &mut menu, &store, &user);

    assert!(menu.status.is_none());
    assert_eq!(menu.worlds.len(), 1);
    assert_eq!(
        menu.worlds[0].map,
        MapType::Procedural {
            seed: 1234,
            size: ProceduralMapSize::Small,
        }
    );

    let _ = fs::remove_dir_all(store.0.root());
}

#[test]
fn rename_world_from_dialog_updates_name_only() {
    let store = temp_store();
    let save = store
        .0
        .create_world_with_map(
            "Original",
            Some(42),
            MapType::Procedural {
                seed: 1234,
                size: ProceduralMapSize::Large,
            },
        )
        .expect("world should create");
    let mut menu = MenuState::default();

    refresh_worlds(&mut menu, &store);
    let mut dialog = EditWorldDialog::new(&menu.worlds[0]);
    dialog.name = "Renamed".to_owned();

    rename_world_from_dialog(dialog, &mut menu, &store);

    assert!(menu.status.is_none());
    assert_eq!(menu.worlds[0].name, "Renamed");
    assert_eq!(menu.worlds[0].id, save.id);
    assert_eq!(
        menu.worlds[0].map,
        MapType::Procedural {
            seed: 1234,
            size: ProceduralMapSize::Large,
        }
    );

    let _ = fs::remove_dir_all(store.0.root());
}

#[test]
fn escape_cancels_modal_or_returns_to_main_menu() {
    let ctx = egui::Context::default();
    let mut menu = MenuState {
        screen: Screen::Worlds,
        create_world: Some(CreateWorldDialog::new()),
        ..Default::default()
    };

    let _ = ctx.run(
        raw_input_with_events(vec![key_press(egui::Key::Escape)]),
        |ctx| {
            super::handle_worlds_escape(ctx, &mut menu);
        },
    );

    let create_dialog = menu
        .create_world
        .expect("dialog should remain while closing");
    assert!(create_dialog.closing);
    assert!(!create_dialog.confirmed);
    assert_eq!(menu.screen, Screen::Worlds);

    let ctx = egui::Context::default();
    let mut menu = MenuState {
        screen: Screen::Worlds,
        ..Default::default()
    };

    let _ = ctx.run(
        raw_input_with_events(vec![key_press(egui::Key::Escape)]),
        |ctx| {
            super::handle_worlds_escape(ctx, &mut menu);
        },
    );

    assert_eq!(menu.screen, Screen::MainMenu);
}

#[test]
fn enter_confirms_create_world_modal() {
    let ctx = egui::Context::default();
    let store = temp_store();
    let user = steam_user();
    let mut menu = MenuState {
        screen: Screen::Worlds,
        create_world: Some(CreateWorldDialog::new()),
        ..Default::default()
    };
    let mut runtime = ClientRuntime::default();

    let _ = ctx.run(
        raw_input_with_events(vec![key_press(egui::Key::Enter)]),
        |ctx| {
            super::worlds_ui(ctx, &mut menu, &mut runtime, &store, &user);
        },
    );

    let create_dialog = menu
        .create_world
        .expect("dialog should remain while closing");
    assert!(create_dialog.closing);
    assert!(create_dialog.confirmed);

    let _ = fs::remove_dir_all(store.0.root());
}

#[test]
fn worlds_ui_renders_empty_and_populated_tables() {
    let ctx = egui::Context::default();
    let store = temp_store();
    let user = steam_user();
    let mut menu = MenuState::default();
    let mut runtime = ClientRuntime::default();

    let _ = ctx.run(raw_input(), |ctx| {
        super::worlds_ui(ctx, &mut menu, &mut runtime, &store, &user);
    });

    store
        .0
        .create_world("Rendered", Some(user.0.steam_id))
        .expect("world should create");
    refresh_worlds(&mut menu, &store);
    assert_eq!(menu.worlds[0].map, MapType::Test);

    let _ = ctx.run(raw_input(), |ctx| {
        super::worlds_ui(ctx, &mut menu, &mut runtime, &store, &user);
    });

    assert!(super::table::table_height(&ctx) >= 180.0);

    let _ = fs::remove_dir_all(store.0.root());
}
