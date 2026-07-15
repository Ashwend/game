//! Dev-only client control socket.
//!
//! Lets an external agent drive the running client (screenshot, slash command,
//! menu navigation, state dump) over a Unix socket, so automated tests can
//! launch the game, act, and assert on JSON state instead of pixels.
//!
//! This is a thin transport adapter, exactly the role the admin socket plays on
//! the server side (`src/net/host/admin.rs`); it owns no gameplay rules, it only
//! pokes existing client resources or forwards a `ClientMessage::Command`.
//!
//! Inert by default: the socket is bound only when `GAME_CONTROL_SOCKET` names a
//! path, so a normal `./cli client` launch never opens it and shipped builds
//! carry zero runtime cost. Unix-only (it uses `UnixListener`); the module is
//! `#[cfg(unix)]`-gated at the `mod` declaration.

use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use serde::{Deserialize, Serialize};

mod targeting;

use self::targeting::{
    building_piece_needle, nearest_deployable_id, parse_building_piece, resolve_building_pose,
};
use super::HeadlessCapture;
use crate::{
    app::state::{ClientRuntime, LocalPlayerState, LookState, MenuState, Screen, WorldMapUiState},
    controller::MAX_LOOK_PITCH,
    items::{ItemModel, intern_item_id, item_definition},
    protocol::{
        ClientMessage, InventoryCommand, PlaceDeployableCommand, SwingStartCommand, Vec3Net,
    },
};

/// Owner+group only, matching the server admin socket. The socket grants full
/// control of the client, so it must stay in a user-private directory.
const CONTROL_SOCKET_MODE: u32 = 0o660;

/// One request from the controlling agent. Tagged JSON, e.g.
/// `{"command":"set_inventory_open","open":true}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum ControlRequest {
    /// Capture the primary window (3D scene + egui UI) to a PNG. Asynchronous:
    /// the file lands a frame or two later, so poll for it before reading.
    Screenshot { path: PathBuf },
    /// Forward a slash command to the server (no leading `/`), e.g. `test-kit`.
    SendCommand { text: String },
    /// Select an actionbar slot (0-based), making that slot's item the held /
    /// active one, exactly as pressing its number key would. Lets an agent put
    /// a specific tool in hand to verify its held viewmodel (e.g. after
    /// `test-kit`, the iron pickaxe lands in slot 3).
    SelectActionbarSlot { slot: usize },
    /// Place a deployable the player is carrying (e.g. `workbench_t1`,
    /// `crude_furnace`) onto level ground a short distance in front of them,
    /// turned to face the player. Position is derived from the local view yaw
    /// rather than the look ray, so it works headless without aiming at the
    /// floor. Lets an agent drop a structure to verify its authored in-world
    /// model. `distance` (metres, default ~2.2) must stay within placement
    /// reach; the server still validates inventory, ground, and overlap.
    PlaceDeployable {
        item_id: String,
        #[serde(default)]
        distance: Option<f32>,
        /// Surface height for the request (a platform top such as a
        /// foundation's `y + 0.5`); defaults to the ground.
        #[serde(default)]
        height: Option<f32>,
    },
    /// Place a building block (`foundation` / `wall` / `window_wall` /
    /// `doorway`) a short distance ahead along the view yaw, like
    /// `PlaceDeployable`. The server snaps the request to the building
    /// grid (walls onto the nearest foundation edge socket), so aim the
    /// distance near a socket rather than exactly on it.
    PlaceBuilding {
        piece: String,
        #[serde(default)]
        distance: Option<f32>,
        /// Raise the request above the ground (free foundations only;
        /// the server validates the raise band and snapped pieces ignore
        /// it). Lets the agent verify stilted foundations headless.
        #[serde(default)]
        height: Option<f32>,
    },
    /// Hang a carried door in the nearest free doorway (within reach),
    /// setting its lock code. `flip` mirrors hinge + swing; `iron` hangs the
    /// iron door variant instead of the default hewn log door (the carried
    /// item must match).
    PlaceDoor {
        code: String,
        #[serde(default)]
        flip: bool,
        #[serde(default)]
        iron: bool,
    },
    /// E-press the nearest door (toggle, or get the code prompt when
    /// unauthorized).
    DoorInteract,
    /// Pick the nearest door back into inventory (hold-E wheel "Pick Up").
    /// Server enforces claim authorization and that the door is unlocked.
    DoorPickUp,
    /// Open the nearest storage box's container UI (the shared loot-bag
    /// transfer panel), like an E-press on the placed box.
    OpenStorageBox,
    /// Close whatever container (loot bag / sleeper / storage box) is
    /// open, like clicking the panel's Close button.
    CloseContainer,
    /// Hammer-upgrade the nearest building block to its next tier. The
    /// server enforces the hammer-in-hand, ownership, and material cost;
    /// select the hammer slot first. `piece` narrows the target to one
    /// piece kind (foundation/wall/...), nearest-of-any otherwise.
    UpgradeBuilding {
        #[serde(default)]
        piece: Option<String>,
    },
    /// Hammer-demolish the nearest building block (optionally narrowed to
    /// one piece kind). Server enforces hammer, ownership, and the
    /// demolish window; structural cascade follows automatically.
    DemolishBuilding {
        #[serde(default)]
        piece: Option<String>,
    },
    /// Enter a code at the nearest door (the first-open authorization).
    DoorEnterCode { code: String },
    /// Point the camera: absolute yaw/pitch in radians, exactly as if the
    /// mouse had moved there. Pitch is clamped to the same limit as mouse
    /// look. Lets an agent aim at ground-level targets (resource nodes,
    /// placed structures) for screenshots and for commands that target
    /// along the view ray (e.g. `/drain`).
    SetLook { yaw: f32, pitch: f32 },
    /// Navigate between menu screens (main_menu / worlds / multiplayer /
    /// options / in_game). Does not start a session; connect via `--connect`.
    SetScreen { screen: String },
    /// Open or close the inventory panel. `admin_tab` additionally lands the
    /// panel on the admin item-grant tab (admins only; the panel forces the
    /// flag off otherwise), so an agent can screenshot it headless.
    SetInventoryOpen {
        open: bool,
        #[serde(default)]
        admin_tab: bool,
    },
    /// Open or close the unified panel on the Crafting tab, standing in for
    /// the C hotkey a headless (unfocused) window can't receive. Opening
    /// clears `inventory_open` the same way the toggle systems keep the two
    /// bools mutually exclusive.
    SetCraftingOpen { open: bool },
    /// Open or close the world-map overlay, bypassing the focus + toggle-key
    /// gate the normal input path uses (the headless window is unfocused, so a
    /// key press can't open it). Opening also fires a `RequestWorldMap` so the
    /// terrain + markers stream in for a screenshot.
    SetWorldMapOpen { open: bool },
    /// Drop a world-map marker at a world (x, z), as if the player had
    /// right-clicked the map there. Lets an agent populate the map to verify
    /// pin rendering headlessly. The server assigns the id and persists it.
    AddWorldMapMarker { x: f32, z: f32 },
    /// Set the world-map pan/zoom viewport directly, standing in for the
    /// wheel-zoom + drag-pan a headless agent can't drive. `zoom` 1.0 fits the
    /// whole world; `center` is the world (x, z) shown at the map centre.
    SetWorldMapView {
        zoom: f32,
        center_x: f32,
        center_z: f32,
    },
    /// Teleport the local player to an absolute world (x, z), keeping the
    /// current height (the server lets gravity settle it). Movement is
    /// client-authoritative, so this just sets the predicted position and the
    /// movement send carries it to peers. Lets an agent stage two players a
    /// fixed distance apart to screenshot one from the other's view.
    Warp { x: f32, z: f32 },
    /// Fire one swing of the currently-held tool (cosmetic): sends a
    /// `SwingStart` so peers play the matching third-person swing on this
    /// player's rigged body. The tool is read from the active actionbar; an
    /// empty hand swings bare-handed. Lets an agent capture the remote swing
    /// animation headless (the normal LMB path is focus-gated).
    Swing,
    /// Throw the held powder bomb along the current look direction at `power`
    /// (charge fraction 0..1, default 1.0). Sends the real
    /// `ExplosiveCommand::Throw`, so the server runs the full consume /
    /// ballistics / bounce / fuse / blast path; only the hold-LMB charge UI is
    /// bypassed (it is focus-gated, like [`Self::Swing`]'s LMB path). Lets an
    /// agent watch a bomb arc, roll, and detonate headless.
    ThrowBomb { power: Option<f32> },
    /// Random-respawn a dead agent (the death splash's Respawn button; the
    /// button itself is unreachable headless). Sends `ClientMessage::Respawn`;
    /// the server no-ops it for a living player.
    Respawn,
    /// Select the actionbar slot that currently holds `item_id` (searches the
    /// replicated actionbar), making it the active/held item. Unlike
    /// [`Self::SelectActionbarSlot`] this doesn't depend on knowing the slot
    /// index, which shifts with the player's loadout. Holding a deployable or the
    /// building plan is what raises the placement ghost, so this lets an agent
    /// start a placement preview headlessly (e.g. `crude_furnace`, `building_plan`).
    SelectActionbarItem { item_id: String },
    /// Equip a wearable piece from the bag/actionbar into its matching
    /// paperdoll slot, standing in for the shift-click quick-equip a headless
    /// agent can't perform (e.g. to verify worn-armor visuals on the rig or
    /// the inventory's character preview). The destination slot resolves from
    /// the piece's `ArmorProfile`; the server still validates the move.
    EquipItem { item_id: String },
    /// Force the local ranged draw/reload state for a frame so an agent can
    /// screenshot the animated bow / crossbow viewmodel poses headless (the real
    /// draw is driven by the focus-gated mouse button). Dev-only, like [`Self::Swing`]:
    /// it writes straight onto [`crate::app::state::RangedDrawState`] via its debug
    /// override, which the pose system reads the same frame. `draw` holds a bow draw
    /// at that fraction (0..1); `reload` sets the crossbow reload crank fraction
    /// (0..1); `recoil` sets the crossbow fire kick (0..1); `aim` holds the
    /// crossbow aim-down-sights fraction (0..1); `swing` freezes the melee swing
    /// fraction (0..1) so a mid-swing viewmodel (a spear thrust, a sword slash)
    /// can be screenshotted. All optional; omitted fields clear (a plain call
    /// with no fields clears every override back to live input).
    RangedPoseDebug {
        draw: Option<f32>,
        reload: Option<f32>,
        recoil: Option<f32>,
        aim: Option<f32>,
        swing: Option<f32>,
        /// Holds a consumable (bandage) use charge at this fraction (0..1), so the
        /// mid-wrap viewmodel and its unrolling tail can be screenshotted.
        #[serde(default)]
        use_charge: Option<f32>,
    },
    /// Hold forward movement input at the current look yaw for `seconds`
    /// (optionally at run speed), so an agent can walk the real world and
    /// exercise collision / step-up end to end. Dev-only, like
    /// [`Self::Swing`]; expires on its own, and a second call replaces the
    /// order (`seconds: 0` cancels).
    Walk { seconds: f32, run: Option<bool> },
    /// Return a JSON snapshot of key client state for assertions.
    DumpState,
}

#[derive(Debug, Serialize)]
struct ControlResponse {
    ok: bool,
    message: String,
}

/// JSON snapshot returned by [`ControlRequest::DumpState`]. Assembled by hand
/// because `ClientRuntime` / `MenuState` aren't `Serialize`; this is the stable
/// shape an agent asserts against.
#[derive(Debug, Serialize)]
struct ClientStateDump {
    /// The placement ghost's world position `[x, y, z]`, or `null` when no
    /// ghost is up (no placeable held, or no valid aim surface). Lets an agent
    /// assert the green/red preview is live without reading pixels.
    ghost_position: Option<[f32; 3]>,
    /// Whether the ghost previews a VALID placement (green) this frame.
    ghost_valid: bool,
    client_id: Option<u64>,
    is_admin: bool,
    world_loaded: bool,
    world_version: u64,
    /// Strong "the world finished loading" signal: connected, world installed,
    /// and the local player's replicated entity has arrived.
    in_world: bool,
    /// Whether the owner-only `PlayerPrivate` (inventory/crafting) replicated.
    /// Distinguishes a fresh-but-empty inventory (Some) from one that never
    /// arrived (None), e.g. after a sleeping-body wake with a stale owner override.
    private_present: bool,
    screen: String,
    inventory_open: bool,
    crafting_open: bool,
    furnace_open: bool,
    loot_bag_open: bool,
    pause_open: bool,
    chat_open: bool,
    death_splash: bool,
    position: Option<[f32; 3]>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    health: Option<f32>,
    local_ping_ms: u16,
    players: Vec<PlayerDump>,
    /// Replicated deployables in AoI (placed structures, building blocks,
    /// doors, bags) so an agent can assert on placements and resolve ids.
    deployables: Vec<DeployableDump>,
    /// The live meteor shower fireball's true world position `[x, y, z]` this frame,
    /// or `null` when no meteor is in flight (no event, not yet in flight, or
    /// already struck). Lets a headless capture aim the camera straight at the
    /// descending object without knowing the trajectory seed. Dev-only, like the
    /// rest of this dump.
    meteor_world: Option<[f32; 3]>,
    /// The live fireball's world-space velocity `[x, y, z]` (m/s), or `null` when
    /// no meteor is in flight. Lets a headless capture stand broadside to the
    /// trajectory so the trail is not occluded behind the ball. Dev-only.
    meteor_velocity: Option<[f32; 3]>,
    /// The announced meteor shower impact point `[x, y, z]`, or `null` when no event
    /// is live. Non-null for the whole event (countdown, flight, crater), unlike
    /// `meteor_world`: lets an agent position itself relative to ground zero
    /// BEFORE the strike (e.g. inside the danger radius for the HUD warning, or
    /// at a safe vantage for the impact) and find the crater afterwards. Dev-only.
    meteor_shower_impact: Option<[f32; 3]>,
}

#[derive(Debug, Serialize)]
struct PlayerDump {
    client_id: u64,
    name: String,
    ping_ms: u16,
}

#[derive(Debug, Serialize, Clone)]
struct DeployableDump {
    id: u64,
    kind: String,
    position: [f32; 3],
    yaw: f32,
    health: u32,
    max_health: u32,
    active: bool,
}

#[derive(Resource)]
pub(crate) struct ClientControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl ClientControlSocket {
    /// Env var that both enables the socket and names its path.
    pub(crate) const ENV: &'static str = "GAME_CONTROL_SOCKET";

    pub(crate) fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create control socket directory {}",
                    parent.display()
                )
            })?;
        }
        remove_stale_socket(&path)?;

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("could not bind control socket {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("could not set control socket to non-blocking")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(CONTROL_SOCKET_MODE))
            .context("could not set control socket permissions")?;

        Ok(Self { listener, path })
    }
}

impl Drop for ClientControlSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Non-blocking drain of pending control requests, one per accepted connection.
/// Registered bare in the `Update` schedule (no ordering dependency) and a
/// no-op whenever the socket resource is absent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_control_socket(
    mut commands: Commands,
    socket: Option<Res<ClientControlSocket>>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut look: ResMut<LookState>,
    mut world_map_ui: ResMut<WorldMapUiState>,
    mut ranged_input: ResMut<crate::app::state::RangedDrawState>,
    mut consume_charge: ResMut<crate::app::state::ConsumeChargeState>,
    mut gather_input: ResMut<crate::app::state::GatherInputState>,
    mut inventory_ui: ResMut<crate::app::state::InventoryUiState>,
    placement: Res<crate::app::state::DeployablePlacementState>,
    local_player: Res<LocalPlayerState>,
    capture: Option<Res<HeadlessCapture>>,
    replicated_deployables: Query<(
        &crate::server::Deployable,
        &crate::server::DeployableTransform,
        &crate::server::DeployableHealth,
        &crate::server::DeployableActive,
    )>,
) {
    let Some(socket) = socket else {
        return;
    };
    let capture = capture.as_deref();

    // Snapshot once per drain; requests are rare (agent-paced) and the
    // AoI deployable set is small.
    let deployables: Vec<DeployableDump> = replicated_deployables
        .iter()
        .map(|(meta, transform, health, active)| DeployableDump {
            id: meta.id,
            kind: format!("{:?}", meta.kind),
            position: [
                transform.position.x,
                transform.position.y,
                transform.position.z,
            ],
            yaw: transform.yaw,
            health: health.0,
            max_health: meta.max_health,
            active: active.0,
        })
        .collect();

    loop {
        let (stream, _) = match socket.listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            Err(error) => {
                eprintln!("could not accept control socket request: {error}");
                return;
            }
        };
        handle_stream(
            stream,
            &mut commands,
            &mut runtime,
            &mut menu,
            &mut look,
            &mut world_map_ui,
            &mut ranged_input,
            &mut consume_charge,
            &mut gather_input,
            &mut inventory_ui,
            &placement,
            &local_player,
            capture,
            &deployables,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_stream(
    mut stream: UnixStream,
    commands: &mut Commands,
    runtime: &mut ClientRuntime,
    menu: &mut MenuState,
    look: &mut LookState,
    world_map_ui: &mut WorldMapUiState,
    ranged_input: &mut crate::app::state::RangedDrawState,
    consume_charge: &mut crate::app::state::ConsumeChargeState,
    gather_input: &mut crate::app::state::GatherInputState,
    inventory_ui: &mut crate::app::state::InventoryUiState,
    placement: &crate::app::state::DeployablePlacementState,
    local_player: &LocalPlayerState,
    capture: Option<&HeadlessCapture>,
    deployables: &[DeployableDump],
) {
    let result = (|| -> Result<String> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let request = serde_json::from_reader(&mut stream)?;
        handle_request(
            request,
            commands,
            runtime,
            menu,
            look,
            world_map_ui,
            ranged_input,
            consume_charge,
            gather_input,
            inventory_ui,
            placement,
            local_player,
            capture,
            deployables,
        )
    })();

    let (ok, message) = match result {
        Ok(message) => (true, message),
        Err(error) => (false, error.to_string()),
    };
    write_response(&mut stream, ok, message);
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
    request: ControlRequest,
    commands: &mut Commands,
    runtime: &mut ClientRuntime,
    menu: &mut MenuState,
    look: &mut LookState,
    world_map_ui: &mut WorldMapUiState,
    ranged_input: &mut crate::app::state::RangedDrawState,
    consume_charge: &mut crate::app::state::ConsumeChargeState,
    gather_input: &mut crate::app::state::GatherInputState,
    inventory_ui: &mut crate::app::state::InventoryUiState,
    placement: &crate::app::state::DeployablePlacementState,
    local_player: &LocalPlayerState,
    capture: Option<&HeadlessCapture>,
    deployables: &[DeployableDump],
) -> Result<String> {
    match request {
        ControlRequest::Screenshot { path } => {
            // In headless-capture mode the primary camera renders to an
            // off-screen image (the window is hidden), so screenshot that image;
            // otherwise read the live window framebuffer as before.
            let screenshot = match capture {
                Some(capture) => Screenshot::image(capture.image.clone()),
                None => Screenshot::primary_window(),
            };
            commands
                .spawn(screenshot)
                .observe(save_to_disk(path.clone()));
            Ok(format!(
                "screenshot queued to {} (lands within a frame or two)",
                path.display()
            ))
        }
        ControlRequest::SendCommand { text } => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Command { text })?;
            Ok("command queued".to_owned())
        }
        ControlRequest::SelectActionbarSlot { slot } => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Inventory(
                InventoryCommand::SelectActionbarSlot { slot },
            ))?;
            Ok(format!("selected actionbar slot {slot}"))
        }
        ControlRequest::SelectActionbarItem { item_id } => {
            let private = local_player
                .private
                .as_ref()
                .context("not in a world (no inventory)")?;
            let holds = |stack: &Option<crate::protocol::ItemStack>| {
                stack.as_ref().map(|s| s.item_id.as_ref()) == Some(item_id.as_str())
            };
            // Prefer an actionbar slot that already holds the item.
            if let Some(slot) = private.inventory.actionbar_slots.iter().position(holds) {
                let session = runtime
                    .session
                    .as_mut()
                    .context("no active session (not in a world)")?;
                session.send(ClientMessage::Inventory(
                    InventoryCommand::SelectActionbarSlot { slot },
                ))?;
                return Ok(format!("selected actionbar slot {slot} ({item_id})"));
            }
            // Fall back to the inventory grid: the test-kit overflows equipables
            // past the ninth into the bag (e.g. the crossbow), so an agent still
            // needs to hold them. Move the stack into a free actionbar slot, then
            // select it. This is a dev-harness convenience only; the server still
            // validates the move.
            let inv_slot = private
                .inventory
                .inventory_slots
                .iter()
                .position(holds)
                .with_context(|| {
                    format!("item '{item_id}' is not in the actionbar or inventory")
                })?;
            let free_actionbar = private
                .inventory
                .actionbar_slots
                .iter()
                .position(Option::is_none)
                .context("no free actionbar slot to move the item into")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            // A distinct seq per move so the server never treats it as stale.
            static MOVE_SEQ: AtomicU32 = AtomicU32::new(1);
            let seq = MOVE_SEQ.fetch_add(1, Ordering::Relaxed);
            session.send(ClientMessage::Inventory(InventoryCommand::Move {
                from: crate::protocol::ItemContainerSlot::inventory(inv_slot),
                to: crate::protocol::ItemContainerSlot::actionbar(free_actionbar),
                quantity: None,
                seq,
            }))?;
            session.send(ClientMessage::Inventory(
                InventoryCommand::SelectActionbarSlot {
                    slot: free_actionbar,
                },
            ))?;
            Ok(format!(
                "moved {item_id} from inventory {inv_slot} to actionbar {free_actionbar} and selected it"
            ))
        }
        ControlRequest::EquipItem { item_id } => {
            let private = local_player
                .private
                .as_ref()
                .context("not in a world (no inventory)")?;
            let profile = crate::items::armor_profile(&item_id)
                .with_context(|| format!("'{item_id}' has no armor profile (not wearable)"))?;
            let holds = |stack: &Option<crate::protocol::ItemStack>| {
                stack.as_ref().map(|s| s.item_id.as_ref()) == Some(item_id.as_str())
            };
            let from = if let Some(slot) = private.inventory.inventory_slots.iter().position(holds)
            {
                crate::protocol::ItemContainerSlot::inventory(slot)
            } else if let Some(slot) = private.inventory.actionbar_slots.iter().position(holds) {
                crate::protocol::ItemContainerSlot::actionbar(slot)
            } else {
                anyhow::bail!("item '{item_id}' is not in the bag or actionbar");
            };
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            // A distinct seq per move so the server never treats it as stale.
            static EQUIP_SEQ: AtomicU32 = AtomicU32::new(1);
            let seq = EQUIP_SEQ.fetch_add(1, Ordering::Relaxed);
            session.send(ClientMessage::Inventory(InventoryCommand::Move {
                from,
                to: crate::protocol::ItemContainerSlot::equipment(profile.slot),
                quantity: None,
                seq,
            }))?;
            Ok(format!(
                "equip queued: {item_id} -> {:?} slot",
                profile.slot
            ))
        }
        ControlRequest::PlaceDeployable {
            item_id,
            distance,
            height,
        } => {
            let view = runtime
                .local_view()
                .context("no local player view (not in a world)")?;
            let dist = distance.unwrap_or(2.2);
            // Player forward is `(-sin yaw, 0, -cos yaw)` (see
            // `controller::movement`), so drop the structure that far ahead on
            // the floor (or the surface at `height`). A deployable's front is
            // +Z, so leaving its yaw equal to the view yaw turns that front
            // back toward the camera.
            let (sin_yaw, cos_yaw) = view.yaw.sin_cos();
            let position = Vec3Net::new(
                view.position.x - sin_yaw * dist,
                height.unwrap_or(0.0),
                view.position.z - cos_yaw * dist,
            );
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::PlaceDeployable(PlaceDeployableCommand {
                item_id: intern_item_id(&item_id),
                position,
                yaw: view.yaw,
                wall_mounted: false,
            }))?;
            Ok(format!(
                "place {item_id} queued at [{:.2}, 0.00, {:.2}]",
                position.x, position.z
            ))
        }
        ControlRequest::PlaceBuilding {
            piece,
            distance,
            height,
        } => {
            let piece = parse_building_piece(&piece)?;
            let view = runtime
                .local_view()
                .context("no local player view (not in a world)")?;
            let dist = distance.unwrap_or(3.0);
            let (sin_yaw, cos_yaw) = view.yaw.sin_cos();
            let aim = Vec3Net::new(
                view.position.x - sin_yaw * dist,
                height.unwrap_or(0.0),
                view.position.z - cos_yaw * dist,
            );
            let (position, yaw) = resolve_building_pose(piece, aim, view.yaw, deployables);
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::PlaceBuilding(
                crate::protocol::PlaceBuildingCommand {
                    piece,
                    position,
                    yaw,
                },
            ))?;
            Ok(format!(
                "place building {piece:?} queued at [{:.2}, {:.2}, {:.2}] (server snaps)",
                position.x, position.y, position.z
            ))
        }
        ControlRequest::PlaceDoor { code, flip, iron } => {
            let doorway = nearest_deployable_id(runtime, deployables, "Doorway")
                .context("no doorway building block in AoI")?;
            let variant = if iron {
                crate::items::DoorVariant::Iron
            } else {
                crate::items::DoorVariant::HewnLog
            };
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Door(crate::protocol::DoorCommand::Place {
                doorway_id: doorway,
                variant,
                flip,
                code,
            }))?;
            Ok(format!("door placement queued in doorway {doorway}"))
        }
        ControlRequest::DoorInteract => {
            let door =
                nearest_deployable_id(runtime, deployables, "Door").context("no door in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Door(
                crate::protocol::DoorCommand::Interact { id: door },
            ))?;
            Ok(format!("door interact queued for {door}"))
        }
        ControlRequest::DoorPickUp => {
            let door =
                nearest_deployable_id(runtime, deployables, "Door").context("no door in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Door(crate::protocol::DoorCommand::PickUp {
                id: door,
            }))?;
            Ok(format!("door pickup queued for {door}"))
        }
        ControlRequest::OpenStorageBox => {
            // Ruin caches share the storage container wire path, so the same
            // request opens whichever is nearer when no box is around; this
            // lets an agent verify cache lootability headlessly.
            let target = nearest_deployable_id(runtime, deployables, "StorageBox")
                .or_else(|| nearest_deployable_id(runtime, deployables, "RuinCache"))
                .context("no storage box or ruin cache in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::OpenStorageBox { id: target })?;
            Ok(format!("container open queued for {target}"))
        }
        ControlRequest::CloseContainer => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::LootBag(
                crate::protocol::LootBagCommand::Close,
            ))?;
            Ok("container close queued".to_owned())
        }
        ControlRequest::UpgradeBuilding { piece } => {
            let needle = building_piece_needle(piece.as_deref())?;
            let target = nearest_deployable_id(runtime, deployables, &needle)
                .context("no matching building block in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Building(
                crate::protocol::BuildingCommand::Upgrade { id: target },
            ))?;
            Ok(format!("upgrade queued for building {target}"))
        }
        ControlRequest::DemolishBuilding { piece } => {
            let needle = building_piece_needle(piece.as_deref())?;
            let target = nearest_deployable_id(runtime, deployables, &needle)
                .context("no matching building block in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Building(
                crate::protocol::BuildingCommand::Demolish { id: target },
            ))?;
            Ok(format!("demolish queued for building {target}"))
        }
        ControlRequest::DoorEnterCode { code } => {
            let door =
                nearest_deployable_id(runtime, deployables, "Door").context("no door in AoI")?;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Door(
                crate::protocol::DoorCommand::EnterCode { id: door, code },
            ))?;
            Ok(format!("door code entry queued for {door}"))
        }
        ControlRequest::SetLook { yaw, pitch } => {
            if !yaw.is_finite() || !pitch.is_finite() {
                bail!("yaw/pitch must be finite radians");
            }
            look.yaw = yaw;
            look.pitch = pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
            Ok(format!(
                "look set to yaw {:.3}, pitch {:.3}",
                look.yaw, look.pitch
            ))
        }
        ControlRequest::SetScreen { screen } => {
            menu.screen = parse_screen(&screen)?;
            Ok(format!("screen set to {:?}", menu.screen))
        }
        ControlRequest::SetInventoryOpen { open, admin_tab } => {
            menu.inventory_open = open;
            inventory_ui.admin_tab = open && admin_tab;
            Ok(format!("inventory_open = {open} (admin_tab = {admin_tab})"))
        }
        ControlRequest::SetCraftingOpen { open } => {
            menu.crafting_open = open;
            if open {
                menu.inventory_open = false;
            }
            Ok(format!("crafting_open = {open}"))
        }
        ControlRequest::SetWorldMapOpen { open } => {
            menu.world_map_open = open;
            if open && let Some(session) = runtime.session.as_mut() {
                // Pull the terrain + markers so the overlay isn't stuck on
                // "Loading map..." in the screenshot.
                session.send(ClientMessage::RequestWorldMap)?;
            }
            Ok(format!("world_map_open = {open}"))
        }
        ControlRequest::AddWorldMapMarker { x, z } => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::WorldMapMarker(
                crate::protocol::WorldMapMarkerCommand::Add { x, z },
            ))?;
            Ok(format!("add marker queued at [{x:.1}, {z:.1}]"))
        }
        ControlRequest::SetWorldMapView {
            zoom,
            center_x,
            center_z,
        } => {
            if !zoom.is_finite() || !center_x.is_finite() || !center_z.is_finite() {
                bail!("zoom/center must be finite");
            }
            world_map_ui.zoom = zoom;
            world_map_ui.center = Some((center_x, center_z));
            Ok(format!(
                "world map view: zoom {zoom:.2}, centre [{center_x:.1}, {center_z:.1}]"
            ))
        }
        ControlRequest::Warp { x, z } => {
            if !x.is_finite() || !z.is_finite() {
                bail!("x/z must be finite");
            }
            let predicted = runtime
                .predicted_local
                .as_mut()
                .context("no local player (not in a world)")?;
            // Keep the current height; the controller + server gravity settle
            // it. Zero momentum so the avatar doesn't keep sliding from the warp.
            predicted.position = Vec3Net::new(x, predicted.position.y, z);
            predicted.velocity = Vec3Net::ZERO;
            Ok(format!("warped to [{x:.2}, {z:.2}]"))
        }
        ControlRequest::Swing => {
            // Derive the swing archetype from the held item exactly as the client
            // and server do: a weapon's own model, a gather tool's archetype, else
            // the bag punch. The server re-derives it too, so this only picks the
            // animation for the headless-harness swing.
            let model = local_player
                .private
                .as_ref()
                .and_then(|private| private.inventory.active_actionbar_stack())
                .and_then(|stack| item_definition(&stack.item_id))
                .map(|definition| definition.swing_model())
                .unwrap_or(ItemModel::Bag);
            // Monotonic per-process seq so the server never rejects it as stale
            // (it keeps the max). One source for all clients in this process is
            // fine: the server dedupes per client_id.
            static SWING_SEQ: AtomicU32 = AtomicU32::new(0);
            let seq = SWING_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::SwingStart(SwingStartCommand { seq, model }))?;
            Ok(format!("swing {model:?} (seq {seq}) sent"))
        }
        ControlRequest::ThrowBomb { power } => {
            let power = power.unwrap_or(1.0).clamp(0.0, 1.0);
            let view = runtime
                .local_view()
                .context("no local player view (not in a world)")?;
            let dir = crate::items::look_forward(view.yaw, view.pitch);
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Explosive(
                crate::protocol::ExplosiveCommand::Throw {
                    aim_dir: Vec3Net::new(dir.x, dir.y, dir.z),
                    power,
                },
            ))?;
            Ok(format!("bomb thrown at power {power:.2}"))
        }
        ControlRequest::Respawn => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Respawn)?;
            // Mirror the death-splash button: drop the splash so the HUD
            // returns the moment the respawned state replicates.
            menu.death_splash = None;
            Ok("respawn requested".to_owned())
        }
        ControlRequest::RangedPoseDebug {
            draw,
            reload,
            recoil,
            aim,
            swing,
            use_charge,
        } => {
            // Force the ranged / swing / consumable pose so the animated bow /
            // crossbow / melee / bandage viewmodel can be screenshotted headless.
            // Clears to live input when every field is None.
            let any_ranged =
                draw.is_some() || reload.is_some() || recoil.is_some() || aim.is_some();
            let over = any_ranged.then_some(crate::app::state::RangedPoseOverride {
                draw,
                reload,
                recoil,
                aim,
            });
            ranged_input.set_debug_override(over);
            gather_input.set_debug_swing_override(swing);
            consume_charge.set_debug_use(use_charge);
            Ok(format!(
                "pose override set (draw={draw:?} reload={reload:?} recoil={recoil:?} aim={aim:?} swing={swing:?} use={use_charge:?})"
            ))
        }
        ControlRequest::Walk { seconds, run } => {
            let run = run.unwrap_or(false);
            let seconds = seconds.clamp(0.0, 30.0);
            look.agent_walk = (seconds > 0.0).then(|| crate::app::state::AgentWalk {
                deadline: std::time::Instant::now() + Duration::from_secs_f32(seconds),
                run,
            });
            Ok(format!("walking forward for {seconds:.1}s (run={run})"))
        }
        ControlRequest::DumpState => {
            let dump = build_dump(runtime, menu, local_player, placement, deployables);
            Ok(serde_json::to_string(&dump)?)
        }
    }
}

fn build_dump(
    runtime: &ClientRuntime,
    menu: &MenuState,
    local_player: &LocalPlayerState,
    placement: &crate::app::state::DeployablePlacementState,
    deployables: &[DeployableDump],
) -> ClientStateDump {
    let view = runtime.local_view();
    ClientStateDump {
        ghost_position: placement.world_position.map(|p| [p.x, p.y, p.z]),
        ghost_valid: placement.valid,
        client_id: runtime.client_id,
        is_admin: runtime.is_admin,
        world_loaded: runtime.world.is_some(),
        world_version: runtime.world_version,
        in_world: runtime.client_id.is_some()
            && runtime.world.is_some()
            && local_player.entity.is_some(),
        private_present: local_player.private.is_some(),
        screen: format!("{:?}", menu.screen),
        inventory_open: menu.inventory_open,
        crafting_open: menu.crafting_open,
        furnace_open: menu.furnace_open,
        loot_bag_open: menu.loot_bag_open,
        pause_open: menu.pause_open,
        chat_open: menu.chat_open,
        death_splash: menu.death_splash.is_some(),
        position: view.map(|v| [v.position.x, v.position.y, v.position.z]),
        yaw: view.map(|v| v.yaw),
        pitch: view.map(|v| v.pitch),
        health: view.map(|v| v.health),
        local_ping_ms: runtime.local_ping_ms,
        players: runtime
            .players
            .iter()
            .map(|p| PlayerDump {
                client_id: p.client_id,
                name: p.name.clone(),
                ping_ms: p.ping_ms,
            })
            .collect(),
        deployables: deployables.to_vec(),
        meteor_world: runtime.meteor_shower.and_then(|event| {
            crate::world::meteor_world_state(
                bevy::math::Vec2::new(event.impact_position.x, event.impact_position.z),
                event.impact_tick,
                event.trajectory_seed,
                runtime.server_tick_precise(),
            )
            .map(|state| [state.position.x, state.position.y, state.position.z])
        }),
        meteor_velocity: runtime.meteor_shower.and_then(|event| {
            crate::world::meteor_world_state(
                bevy::math::Vec2::new(event.impact_position.x, event.impact_position.z),
                event.impact_tick,
                event.trajectory_seed,
                runtime.server_tick_precise(),
            )
            .map(|state| [state.velocity.x, state.velocity.y, state.velocity.z])
        }),
        meteor_shower_impact: runtime.meteor_shower.map(|event| {
            [
                event.impact_position.x,
                event.impact_position.y,
                event.impact_position.z,
            ]
        }),
    }
}

/// Map an agent-supplied screen name to a [`Screen`]. Tolerant of case and of
/// `_`/`-`/space separators so `"main_menu"`, `"MainMenu"`, and `"in game"` all
/// work.
fn parse_screen(raw: &str) -> Result<Screen> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['_', '-', ' '], "");
    Ok(match normalized.as_str() {
        "mainmenu" | "menu" | "main" => Screen::MainMenu,
        "worlds" => Screen::Worlds,
        "multiplayer" => Screen::Multiplayer,
        "options" => Screen::Options,
        "ingame" | "game" => Screen::InGame,
        other => bail!("unknown screen '{other}'"),
    })
}

fn write_response(stream: &mut UnixStream, ok: bool, message: String) {
    let response = ControlResponse { ok, message };
    if let Err(error) = serde_json::to_writer(&mut *stream, &response) {
        eprintln!("could not write control socket response: {error}");
        return;
    }
    let _ = stream.write_all(b"\n");
}

fn remove_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path) {
        Ok(_) => bail!("control socket {} is already in use", path.display()),
        Err(_) => fs::remove_file(path)
            .with_context(|| format!("could not remove stale control socket {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A short, unique temp socket path. macOS caps Unix-domain socket paths at
    /// ~104 bytes (`sockaddr_un::sun_path`), and the default temp dir is already
    /// ~50 chars, so a full UUID-suffixed name overflows and `bind` fails with
    /// `ENAMETOOLONG`. A truncated suffix stays comfortably under the limit while
    /// keeping per-test uniqueness.
    fn temp_socket_path(tag: &str) -> PathBuf {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        std::env::temp_dir().join(format!("ash-{tag}-{}.sock", &unique[..12]))
    }

    #[test]
    fn parse_screen_accepts_aliases_and_separators() {
        assert!(matches!(parse_screen("main_menu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("MainMenu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("  worlds "), Ok(Screen::Worlds)));
        assert!(matches!(parse_screen("in game"), Ok(Screen::InGame)));
        assert!(matches!(
            parse_screen("multiplayer"),
            Ok(Screen::Multiplayer)
        ));
        assert!(parse_screen("nonsense").is_err());
    }

    #[test]
    fn requests_deserialize_from_the_agent_wire_format() {
        // Pin the exact JSON an agent will send so the wire shape can't drift.
        let inv: ControlRequest =
            serde_json::from_str(r#"{"command":"set_inventory_open","open":true}"#).unwrap();
        assert!(matches!(
            inv,
            ControlRequest::SetInventoryOpen {
                open: true,
                admin_tab: false
            }
        ));

        let cmd: ControlRequest =
            serde_json::from_str(r#"{"command":"send_command","text":"test-kit"}"#).unwrap();
        assert!(matches!(cmd, ControlRequest::SendCommand { text } if text == "test-kit"));

        let slot: ControlRequest =
            serde_json::from_str(r#"{"command":"select_actionbar_slot","slot":3}"#).unwrap();
        assert!(matches!(
            slot,
            ControlRequest::SelectActionbarSlot { slot: 3 }
        ));

        let shot: ControlRequest =
            serde_json::from_str(r#"{"command":"screenshot","path":"/tmp/a.png"}"#).unwrap();
        assert!(matches!(shot, ControlRequest::Screenshot { .. }));

        let dump: ControlRequest = serde_json::from_str(r#"{"command":"dump_state"}"#).unwrap();
        assert!(matches!(dump, ControlRequest::DumpState));

        let look: ControlRequest =
            serde_json::from_str(r#"{"command":"set_look","yaw":1.5,"pitch":-0.42}"#).unwrap();
        assert!(matches!(
            look,
            ControlRequest::SetLook { yaw, pitch }
                if (yaw - 1.5).abs() < f32::EPSILON && (pitch + 0.42).abs() < f32::EPSILON
        ));

        // `distance` is optional and defaults to None when omitted.
        let place: ControlRequest =
            serde_json::from_str(r#"{"command":"place_deployable","item_id":"crude_furnace"}"#)
                .unwrap();
        assert!(matches!(
            place,
            ControlRequest::PlaceDeployable { item_id, distance: None, height: None } if item_id == "crude_furnace"
        ));
        let place_dist: ControlRequest = serde_json::from_str(
            r#"{"command":"place_deployable","item_id":"workbench_t1","distance":3.0}"#,
        )
        .unwrap();
        assert!(matches!(
            place_dist,
            ControlRequest::PlaceDeployable { distance: Some(d), .. } if (d - 3.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn bind_creates_then_drop_removes_the_socket() {
        let path = temp_socket_path("ctl");
        let socket = ClientControlSocket::bind(&path).expect("bind should succeed");
        assert!(path.exists(), "socket file should exist while bound");
        drop(socket);
        assert!(!path.exists(), "Drop should remove the socket file");
    }

    #[test]
    fn bind_reclaims_a_stale_socket_file() {
        let path = temp_socket_path("stale");
        // A leftover file with no listener behind it should be reclaimed.
        std::fs::write(&path, b"stale").unwrap();
        let socket = ClientControlSocket::bind(&path).expect("stale socket should be reclaimed");
        assert!(path.exists());
        drop(socket);
        let _ = std::fs::remove_file(&path);
    }
}
