use std::{thread, time::Duration};

use bevy::prelude::*;

use super::*;
use crate::{
    auth::{AuthMode, AuthenticatedUser},
    protocol::{ClientMessage, ServerMessage},
    save::{WorldSave, WorldStore},
    server::ServerSettings,
};

fn user() -> AuthenticatedUser {
    AuthenticatedUser {
        account_id: crate::protocol::AccountId(1),
        display_name: "Host".to_owned(),
        token: String::new(),
    }
}

fn temp_store() -> WorldStore {
    WorldStore::new(
        std::env::temp_dir().join(format!("game-network-test-{}", uuid::Uuid::new_v4())),
    )
}

/// Test rig: a minimal Bevy `App` with the Lightyear client plugins, paired
/// with a `ClientSession`. After Phase 3 of the replication migration the
/// connection lifecycle lives in the main app's `Update` schedule, so each
/// test now drives `app.update()` on this rig to make handshake progress.
struct TestRig {
    app: App,
    session: ClientSession,
}

impl TestRig {
    fn singleplayer() -> Self {
        let user = user();
        let app = build_test_app();
        let network = app.world().resource::<ClientNetwork>().clone();
        let session = ClientSession::start_singleplayer(
            WorldSave::new("Local", Some(user.account_id)),
            &temp_store(),
            &user,
            network,
        )
        .expect("network session should start");
        Self { app, session }
    }

    fn direct(addr: std::net::SocketAddr) -> Self {
        let user = user();
        let app = build_test_app();
        let network = app.world().resource::<ClientNetwork>().clone();
        let session = ClientSession::connect(addr, &user, network)
            .expect("direct network session should connect");
        Self { app, session }
    }

    /// Drive the main app forward one frame.
    fn tick_app(&mut self) {
        self.app.update();
    }

    /// Tick the app and drain anything the receive system pushed into the
    /// shared inbox. Mirrors the shape of the old standalone `session.tick`.
    fn poll(&mut self) -> Vec<ServerMessage> {
        self.tick_app();
        self.session
            .tick(0.0)
            .expect("session should drain its inbox")
    }
}

fn build_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Bevy 0.19: lightyear's ClientPlugins calls init_state, which needs the
    // StateTransition schedule from StatesPlugin (the real client app gets it
    // via DefaultPlugins; this headless test client uses MinimalPlugins).
    app.add_plugins(bevy::state::app::StatesPlugin);
    app.add_plugins(client_plugins());
    app.add_plugins(LightyearProtocolPlugin);
    app.add_plugins(ClientNetworkPlugin);
    app.finish();
    app.cleanup();
    app
}

fn collect_until(
    rig: &mut TestRig,
    accepts: impl Fn(&[ServerMessage]) -> bool,
) -> Vec<ServerMessage> {
    let mut messages = Vec::new();
    for _ in 0..200 {
        messages.extend(rig.poll());
        if accepts(&messages) {
            return messages;
        }
        thread::sleep(Duration::from_millis(10));
    }
    messages
}

#[test]
fn singleplayer_session_connects_through_loopback_server() {
    let mut rig = TestRig::singleplayer();

    let messages = collect_until(&mut rig, |messages| {
        messages
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
    });
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
    );
}

// Deleted: `singleplayer_session_receives_authoritative_snapshots_from_loopback_host`
// was verifying that the `ServerMessage::Snapshot` payload arrived. Phase
// 6.6 retired the snapshot wire path; world state now flows through
// Lightyear's per-component replication and asserting on it requires the
// full plugin set in the test harness.

#[test]
fn singleplayer_chat_round_trips_through_network_server() {
    let mut rig = TestRig::singleplayer();

    // Make sure we've got Welcome and a connected client before sending.
    let _ = collect_until(&mut rig, |messages| {
        messages
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
    });

    rig.session
        .send(ClientMessage::Chat {
            text: "  hello  ".to_owned(),
        })
        .expect("chat should send");

    let messages = collect_until(&mut rig, |messages| {
        messages.iter().any(|message| {
            matches!(
                message,
                ServerMessage::Chat(chat) if chat.from == "Host" && chat.text == "hello"
            )
        })
    });
    assert!(messages.iter().any(|message| {
        matches!(
            message,
            ServerMessage::Chat(chat) if chat.from == "Host" && chat.text == "hello"
        )
    }));
}

#[test]
fn direct_multiplayer_connects_to_shared_lightyear_server_host() {
    let user = user();
    let mut spawned = super::host::spawn_loopback_server(
        WorldSave::new("Remote", Some(user.account_id)),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: None,
        },
        None,
    )
    .expect("Lightyear server should start");

    let mut rig = TestRig::direct(spawned.addr);

    let initial = collect_until(&mut rig, |messages| {
        messages
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
    });
    assert!(
        initial
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
    );

    rig.session
        .send(ClientMessage::Chat {
            text: "  remote path  ".to_owned(),
        })
        .expect("chat should send");

    let messages = collect_until(&mut rig, |messages| {
        messages.iter().any(|message| {
            matches!(
                message,
                ServerMessage::Chat(chat)
                    if chat.from == "Host" && chat.text == "remote path"
            )
        })
    });
    assert!(messages.iter().any(|message| {
        matches!(
            message,
            ServerMessage::Chat(chat) if chat.from == "Host" && chat.text == "remote path"
        )
    }));

    rig.session
        .send(ClientMessage::Disconnect)
        .expect("disconnect should send");
    spawned.handle.shutdown().expect("server should stop");
}

/// Per-entity replication smoke test: a connected client must actually receive
/// replicated world entities in its ECS world, not just `ServerMessage` chat /
/// welcome. The world generator seeds resource nodes around the origin, so once
/// the AoI room subscription lands they replicate into the client world via the
/// per-entity `ReplicationGroup` machinery. This is the cheapest guard that the
/// `ReplicationGroup::new_from_entity()` wiring (upstream bug #740) is delivering
/// entities at all; a regression that breaks the spawn path turns this red
/// instead of only showing up as missing nodes in a live session. (Post-spawn
/// *diff* delivery, a different failure mode, is covered by its companion
/// `replicated_component_post_spawn_diff_reaches_the_client_world` below; see
/// the Replication section of docs/networking.md.)
#[test]
fn replicated_world_entities_reach_the_client_world() {
    let user = user();
    let mut spawned = super::host::spawn_loopback_server(
        WorldSave::new("Replicated", Some(user.account_id)),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: None,
        },
        None,
    )
    .expect("Lightyear server should start");

    let mut rig = TestRig::direct(spawned.addr);

    let mut replicated_nodes = 0usize;
    for _ in 0..400 {
        let _ = rig.poll();
        let world = rig.app.world_mut();
        let mut query = world.query::<&crate::server::ResourceNode>();
        replicated_nodes = query.iter(world).count();
        if replicated_nodes > 0 {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        replicated_nodes > 0,
        "expected replicated ResourceNode entities to reach the client world via the \
         per-entity ReplicationGroup path, found none"
    );

    rig.session
        .send(ClientMessage::Disconnect)
        .expect("disconnect should send");
    spawned.handle.shutdown().expect("server should stop");
}

/// Post-spawn replication-diff guard. The single most-documented failure mode
/// in CLAUDE.md is a server-side component mutation that ships its initial
/// spawn but never delivers the *later* diff (the Lightyear 0.26.4 group-ack
/// dropout that `ReplicationGroup::new_from_entity()` exists to fix). The
/// companion test above proves entities *arrive*; this proves a field that
/// changes *after* spawn actually reaches the client world.
///
/// We mutate a replicated component through normal gameplay: a chat line sets
/// the speaker's `PlayerChatBubble`, which the mirror sync writes onto the
/// player entity and Lightyear must ship as a post-spawn diff. We first wait
/// for the player entity to replicate in with its initial empty bubble (so what
/// follows is provably a diff, not the spawn snapshot), then assert the mutated
/// value lands client-side. Without this, a regression that breaks post-spawn
/// diffing for furnace burn state, door open/closed, deployable health, etc.
/// would pass CI green and surface only as stale state in a live session. The
/// test harness adds no client-side apply systems, so the value observed here
/// is exactly what Lightyear delivered, not a locally-reconstructed view.
#[test]
fn replicated_component_post_spawn_diff_reaches_the_client_world() {
    let user = user();
    let mut spawned = super::host::spawn_loopback_server(
        WorldSave::new("DiffWorld", Some(user.account_id)),
        ServerSettings {
            auth_mode: AuthMode::NoAuth,
            singleplayer_host: None,
        },
        None,
    )
    .expect("Lightyear server should start");

    let mut rig = TestRig::direct(spawned.addr);

    // Wait until the player mirror entity replicates in carrying its initial
    // (empty) chat bubble. Observing the `None` first is what makes the later
    // assertion a genuine post-spawn diff rather than the spawn snapshot.
    let mut empty_bubble_replicated = false;
    for _ in 0..400 {
        let _ = rig.poll();
        let world = rig.app.world_mut();
        let mut query = world.query::<&crate::server::PlayerChatBubble>();
        if query.iter(world).any(|bubble| bubble.0.is_none()) {
            empty_bubble_replicated = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        empty_bubble_replicated,
        "expected the player mirror entity (with an empty PlayerChatBubble) to \
         replicate into the client world before mutating it"
    );

    // Mutate the replicated field server-side via normal gameplay.
    rig.session
        .send(ClientMessage::Chat {
            text: "diff me".to_owned(),
        })
        .expect("chat should send");

    let mut diff_reached_client = false;
    for _ in 0..400 {
        let _ = rig.poll();
        let world = rig.app.world_mut();
        let mut query = world.query::<&crate::server::PlayerChatBubble>();
        if query
            .iter(world)
            .any(|bubble| bubble.0.as_deref() == Some("diff me"))
        {
            diff_reached_client = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        diff_reached_client,
        "expected the post-spawn PlayerChatBubble mutation to reach the client \
         world; a server-side mutate with no client receive is the Lightyear \
         0.26.4 group-ack dropout that ReplicationGroup::new_from_entity guards"
    );

    rig.session
        .send(ClientMessage::Disconnect)
        .expect("disconnect should send");
    spawned.handle.shutdown().expect("server should stop");
}

#[test]
fn singleplayer_shutdown_persists_world_from_network_server() {
    let store = temp_store();
    let user = user();
    let save = store
        .create_world("Persisted", Some(user.account_id))
        .expect("world should create");
    let world_id = save.id;
    let mut app = build_test_app();
    let network = app.world().resource::<ClientNetwork>().clone();
    let mut session = ClientSession::start_singleplayer(save, &store, &user, network)
        .expect("network session should start");

    // Drive the app until Welcome arrives so the loopback server's world
    // state is fully initialised before we ask for a save.
    for _ in 0..200 {
        app.update();
        let messages = session.tick(0.0).expect("session should tick");
        if messages
            .iter()
            .any(|message| matches!(message, ServerMessage::Welcome { .. }))
        {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    // shutdown blocks waiting for the main app to drive the netcode
    // disconnect to completion; drive it on this thread in parallel.
    let session_thread = thread::spawn(move || {
        let store = store;
        let result = session.shutdown(&store);
        (store, result)
    });

    // Pump the app until the shutdown worker thread has completed.
    for _ in 0..600 {
        app.update();
        if session_thread.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let (store, result) = session_thread
        .join()
        .expect("session shutdown thread should join");
    result.expect("session should persist and shut down");

    let loaded = store.load_world(world_id).expect("world should load");
    assert_eq!(loaded.name, "Persisted");
    let _ = std::fs::remove_dir_all(store.root());
}
