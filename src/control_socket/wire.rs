//! Serde shapes on the control-socket wire: the tagged-JSON [`ControlRequest`],
//! the `{ok, message}` [`ControlResponse`] reply, and the state-dump structs an
//! agent asserts against. Pure data, no transport or gameplay logic.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
pub(crate) struct ControlResponse {
    pub(crate) ok: bool,
    pub(crate) message: String,
}

/// JSON snapshot returned by [`ControlRequest::DumpState`]. Assembled by hand
/// because `ClientRuntime` / `MenuState` aren't `Serialize`; this is the stable
/// shape an agent asserts against.
#[derive(Debug, Serialize)]
pub(crate) struct ClientStateDump {
    /// The placement ghost's world position `[x, y, z]`, or `null` when no
    /// ghost is up (no placeable held, or no valid aim surface). Lets an agent
    /// assert the green/red preview is live without reading pixels.
    pub(crate) ghost_position: Option<[f32; 3]>,
    /// Whether the ghost previews a VALID placement (green) this frame.
    pub(crate) ghost_valid: bool,
    pub(crate) client_id: Option<crate::protocol::ClientId>,
    pub(crate) is_admin: bool,
    pub(crate) world_loaded: bool,
    pub(crate) world_version: u64,
    /// Strong "the world finished loading" signal: connected, world installed,
    /// and the local player's replicated entity has arrived.
    pub(crate) in_world: bool,
    /// Whether the owner-only `PlayerPrivate` (inventory/crafting) replicated.
    /// Distinguishes a fresh-but-empty inventory (Some) from one that never
    /// arrived (None), e.g. after a sleeping-body wake with a stale owner override.
    pub(crate) private_present: bool,
    pub(crate) screen: String,
    pub(crate) inventory_open: bool,
    pub(crate) crafting_open: bool,
    pub(crate) furnace_open: bool,
    pub(crate) loot_bag_open: bool,
    pub(crate) pause_open: bool,
    pub(crate) chat_open: bool,
    pub(crate) death_splash: bool,
    pub(crate) position: Option<[f32; 3]>,
    pub(crate) yaw: Option<f32>,
    pub(crate) pitch: Option<f32>,
    pub(crate) health: Option<f32>,
    pub(crate) local_ping_ms: u16,
    pub(crate) players: Vec<PlayerDump>,
    /// Replicated deployables in AoI (placed structures, building blocks,
    /// doors, bags) so an agent can assert on placements and resolve ids.
    pub(crate) deployables: Vec<DeployableDump>,
    /// The live meteor shower fireball's true world position `[x, y, z]` this frame,
    /// or `null` when no meteor is in flight (no event, not yet in flight, or
    /// already struck). Lets a headless capture aim the camera straight at the
    /// descending object without knowing the trajectory seed. Dev-only, like the
    /// rest of this dump.
    pub(crate) meteor_world: Option<[f32; 3]>,
    /// The live fireball's world-space velocity `[x, y, z]` (m/s), or `null` when
    /// no meteor is in flight. Lets a headless capture stand broadside to the
    /// trajectory so the trail is not occluded behind the ball. Dev-only.
    pub(crate) meteor_velocity: Option<[f32; 3]>,
    /// The announced meteor shower impact point `[x, y, z]`, or `null` when no event
    /// is live. Non-null for the whole event (countdown, flight, crater), unlike
    /// `meteor_world`: lets an agent position itself relative to ground zero
    /// BEFORE the strike (e.g. inside the danger radius for the HUD warning, or
    /// at a safe vantage for the impact) and find the crater afterwards. Dev-only.
    pub(crate) meteor_shower_impact: Option<[f32; 3]>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PlayerDump {
    pub(crate) client_id: crate::protocol::ClientId,
    pub(crate) name: String,
    pub(crate) ping_ms: u16,
}

#[derive(Debug, Serialize, Clone)]
pub(crate) struct DeployableDump {
    pub(crate) id: crate::protocol::DeployedEntityId,
    pub(crate) kind: String,
    pub(crate) position: [f32; 3],
    pub(crate) yaw: f32,
    pub(crate) health: u32,
    pub(crate) max_health: u32,
    pub(crate) active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
