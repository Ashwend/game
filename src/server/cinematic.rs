//! Server-side cinematic playback orchestrator.
//!
//! The admin chat command `/cinematic` (cinematic-stage worlds only) runs a
//! scripted marketing sequence entirely through the ordinary authoritative
//! pipeline: an init phase cleans the world and spawns the stage (pre-built
//! base, props, synthetic "dummy" players), then a phase machine walks the
//! shared shot script (`crate::cinematic::script`), broadcasting one
//! `ServerMessage::Cinematic` cue per phase edge. Clients react by detaching
//! the camera and drawing the countdown slate; everything the camera sees
//! (walking actors, swings, the meteor, the PvP kill) is real replicated
//! state, so multiple clients could film the same take from different angles.
//!
//! Dummy actors are synthetic `ServerClient` entries (`synthetic: true`, no
//! transport): the existing player mirror replicates them and remote clients
//! render full rigs, held items, swing animations, and nametags without any
//! client-side changes. The orchestrator writes their pose (nonzero velocity
//! while walking, that is what drives peer walk cycles), bumps `swing_seq`
//! on a work cadence, and keeps `last_seen_tick` fresh so the stale sweep
//! ignores them. They are excluded from persistence and removed when
//! playback stops.

use bevy::math::Vec2;

use crate::cinematic::layout::{
    self, ActorRole, ActorSpec, CellEdge, StageBuildingBlock, StagePropKind,
};
use crate::cinematic::script::{
    self, COUNTDOWN_SECONDS, HARVEST_ORE_SWINGS_LEFT, HARVEST_SHOT_INDEX, HARVEST_TREE_SWINGS_LEFT,
    HOMESTEAD_SHOT_INDEX, INIT_SECONDS, INTERMISSION_SECONDS, METEOR_SHOT_INDEX,
    METEOR_TRAJECTORY_SEED, METEOR_WARNING_SECONDS, SHOTS, SKIRMISH_KILL_SECONDS,
    SKIRMISH_SHOT_INDEX,
};
use crate::items::{
    DeployableKind, DoorVariant, ItemModel, equipped_protection, intern_item_id, item_definition,
};
use crate::protocol::{
    AccountId, CinematicCue, ClientId, DeployedEntityId, ItemStack, PlayerEvent,
    PlayerInventoryState, PlayerState, SERVER_TICK_RATE_HZ, ServerMessage, ToastKind, ToastMessage,
    Vec3Net,
};
use crate::world::MapType;

use super::{
    DeliveryTarget, GameServer, PlayerLifecycle, ServerClient, ServerEnvelope,
    commands::reply_warning, deployables::DeployedEntity,
};

/// Base of the reserved synthetic account-id range. Far above anything a
/// real identity system issues, so a dummy can never collide with (or wake)
/// a real player's sleeping body.
const ACTOR_ACCOUNT_BASE: u64 = 0xC14E_AC70_0000_0000;

/// Movement speeds for scripted actors, tuned to read calm on the remote
/// walk-cycle animator (which scales stride with replicated velocity).
const ACTOR_WALK_SPEED: f32 = 3.4;

/// How close an actor must be to a work spot before it stops and swings.
const WORK_ARRIVE_M: f32 = 0.35;
/// Preferred standing distance from a work target (tree, vein, wall).
const WORK_STAND_OFF_M: f32 = 1.7;

fn secs_to_ticks(seconds: f32) -> u32 {
    ((seconds * SERVER_TICK_RATE_HZ).round() as u32).max(1)
}

/// Facing yaw so the controller's forward vector `(-sin yaw, -cos yaw)`
/// points from `from` toward `(to_x, to_z)`.
fn face_yaw(from: Vec3Net, to_x: f32, to_z: f32) -> f32 {
    let dx = to_x - from.x;
    let dz = to_z - from.z;
    if dx.abs() < f32::EPSILON && dz.abs() < f32::EPSILON {
        return 0.0;
    }
    (-dx).atan2(-dz)
}

fn cue(cue: CinematicCue) -> ServerEnvelope {
    ServerEnvelope {
        target: DeliveryTarget::Broadcast,
        message: ServerMessage::Cinematic(cue),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) enum CinematicPhase {
    #[default]
    Idle,
    Initializing {
        ticks_left: u32,
    },
    Countdown {
        shot: usize,
        ticks_left: u32,
    },
    Playing {
        shot: usize,
        elapsed_ticks: u32,
    },
    Intermission {
        next_shot: Option<usize>,
        ticks_left: u32,
    },
}

#[derive(Debug)]
struct ActorRuntime {
    client_id: ClientId,
    spec: &'static ActorSpec,
    /// Server tick the next work/combat swing fires.
    next_swing_tick: u64,
    /// Wanderer: current waypoint index. Miner: current node index.
    route_index: usize,
    /// Miner: tick to switch to the other node.
    work_switch_tick: u64,
    dead: bool,
    /// Fighter combat-loop state (ignored by the other roles).
    fight_move: FightMove,
    /// Server tick the current fight move expires.
    fight_until: u64,
    /// Circling direction, `1.0` or `-1.0`; flips as the fight evolves.
    circle_dir: f32,
    /// Vertical hop velocity; nonzero while airborne.
    vertical_velocity: f32,
}

/// One beat of the fighter combat loop.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum FightMove {
    #[default]
    Circle,
    Lunge,
    Retreat,
}

#[derive(Debug, Default)]
pub(super) struct CinematicState {
    pub(super) phase: CinematicPhase,
    actors: Vec<ActorRuntime>,
    /// World-time multiplier before playback froze it, restored on stop.
    saved_time_multiplier: f32,
    /// Next pending step of the homestead live build-out.
    build_step: usize,
}

impl CinematicState {
    pub(super) fn active(&self) -> bool {
        self.phase != CinematicPhase::Idle
    }
}

impl GameServer {
    /// `/cinematic [play|stop]`.
    pub(super) fn command_cinematic(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }
        match args.first().copied() {
            None | Some("play") => self.start_cinematic(client_id),
            Some("stop") => {
                if self.cinematic.active() {
                    self.stop_cinematic(false)
                } else {
                    reply_warning(client_id, "no cinematic is playing")
                }
            }
            _ => reply_warning(client_id, "usage: /cinematic [play|stop]"),
        }
    }

    fn start_cinematic(&mut self, issuer: ClientId) -> Vec<ServerEnvelope> {
        if !matches!(self.save.map, MapType::Cinematic) {
            return reply_warning(
                issuer,
                "cinematic playback needs a Cinematic Stage world (create one from the Worlds menu)",
            );
        }
        if self.cinematic.active() {
            return reply_warning(issuer, "already playing; /cinematic stop first");
        }

        let mut envelopes = Vec::new();

        // Init phase: strip transient clutter and any prior stage so replays
        // are byte-identical, then rebuild the stage from the shared layout.
        self.cinematic_cleanup_world();
        envelopes.extend(self.suspend_meteor_events());
        // Replays must find the composed grove fresh: refill drained nodes
        // and respawn anything an earlier take felled.
        self.restore_stage_nodes();
        self.cinematic.build_step = 0;
        self.spawn_cinematic_stage();
        envelopes.extend(self.spawn_cinematic_actors());

        // Freeze the day/night clock so each shot's lighting holds; the
        // per-shot hour is set at every countdown edge.
        self.cinematic.saved_time_multiplier = self.world_time.multiplier;
        self.set_world_time_multiplier(0.0);
        self.set_world_time_seconds(SHOTS[0].world_time_hours * 3600.0);
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::WorldTime(self.world_time_snapshot()),
        });

        // Park every real player at the stage anchor so the whole stage sits
        // inside their AoI ring while the detached cameras fly. Emitted AFTER
        // the WorldTime push above: both ride the sequenced-unreliable
        // channel, where a same-tick newer message drops an older one, so the
        // warp must be the batch's newest. The init phase re-stamps anyone
        // whose echoed position hasn't converged (see `tick_cinematic`), so a
        // dropped Correction heals within a second.
        bevy::log::info!("cinematic: starting playback, warping players to the stage anchor");
        envelopes.extend(self.converge_cinematic_warp());

        self.cinematic.phase = CinematicPhase::Initializing {
            ticks_left: secs_to_ticks(INIT_SECONDS),
        };
        envelopes.push(cue(CinematicCue::Initializing));
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(issuer),
            message: ServerMessage::Toast(ToastMessage::new(
                ToastKind::Success,
                format!(
                    "Cinematic starting: {} shots, ~{:.0} min. /cinematic stop aborts.",
                    SHOTS.len(),
                    cinematic_total_seconds() / 60.0,
                ),
            )),
        });
        envelopes
    }

    /// Tear playback down: despawn actors, restore the clock, re-roll the
    /// meteor scheduler, and cue clients to restore control. The stage
    /// (base, props, authored nodes) stays standing; the next `/cinematic
    /// play` rebuilds it from scratch anyway.
    fn stop_cinematic(&mut self, finished: bool) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        for runtime in std::mem::take(&mut self.cinematic.actors) {
            let Some(client) = self.clients.remove(&runtime.client_id) else {
                continue;
            };
            self.account_to_client.remove(&client.account_id);
            self.chunk_manager.untrack_player(runtime.client_id);
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::PlayerEvent(PlayerEvent::Left {
                    client_id: runtime.client_id,
                    name: client.name,
                }),
            });
        }

        self.set_world_time_multiplier(self.cinematic.saved_time_multiplier.max(0.0));
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Broadcast,
            message: ServerMessage::WorldTime(self.world_time_snapshot()),
        });
        self.meteor_shower = super::meteor_shower::MeteorShowerState::new(
            self.tick,
            self.chunk_manager.world_seed(),
        );

        self.cinematic.phase = CinematicPhase::Idle;
        envelopes.push(cue(CinematicCue::Stopped));
        envelopes.extend(self.announce(if finished {
            "Cinematic finished."
        } else {
            "Cinematic stopped."
        }));
        envelopes
    }

    /// Per-tick playback engine, called from `GameServer::tick` beside the
    /// other world-event ticks. Drives the actor choreography every tick and
    /// advances the phase machine, emitting one cue per edge.
    pub(super) fn tick_cinematic(&mut self) -> Vec<ServerEnvelope> {
        if !self.cinematic.active() {
            return Vec::new();
        }
        let mut envelopes = self.drive_cinematic_actors();

        match self.cinematic.phase {
            CinematicPhase::Idle => {}
            CinematicPhase::Initializing { ticks_left } => {
                // The stage-anchor warp rides the droppable sequenced
                // channel; keep re-stamping stragglers until everyone's
                // echoed position confirms the snap landed.
                if ticks_left.is_multiple_of(20) {
                    envelopes.extend(self.converge_cinematic_warp());
                }
                if ticks_left <= 1 {
                    envelopes.extend(self.enter_cinematic_countdown(0));
                } else {
                    self.cinematic.phase = CinematicPhase::Initializing {
                        ticks_left: ticks_left - 1,
                    };
                }
            }
            CinematicPhase::Countdown { shot, ticks_left } => {
                if ticks_left <= 1 {
                    self.cinematic.phase = CinematicPhase::Playing {
                        shot,
                        elapsed_ticks: 0,
                    };
                    envelopes.push(cue(CinematicCue::ShotStarted {
                        shot_index: shot as u8,
                    }));
                    // The harvest shot must show the tree FALL and the ore
                    // BREAK: cap both nodes' storage AT SHOT START (not the
                    // countdown, whose swings would burn the budget early) so
                    // the real gathers finish them mid-shot, on camera.
                    if shot == HARVEST_SHOT_INDEX {
                        self.pre_drain_node_for_swings(
                            layout::GROVE_HERO_TREE,
                            crate::items::IRON_HATCHET_ID,
                            HARVEST_TREE_SWINGS_LEFT,
                        );
                        self.pre_drain_node_for_swings(
                            layout::GROVE_IRON_NODE,
                            crate::items::IRON_PICKAXE_ID,
                            HARVEST_ORE_SWINGS_LEFT,
                        );
                    }
                    if shot == METEOR_SHOT_INDEX {
                        envelopes.extend(self.force_cinematic_meteor(
                            Vec3Net::new(layout::METEOR_IMPACT.x, 0.0, layout::METEOR_IMPACT.y),
                            METEOR_WARNING_SECONDS,
                            layout::METEOR_SIZE,
                            METEOR_TRAJECTORY_SEED,
                        ));
                    }
                } else {
                    self.cinematic.phase = CinematicPhase::Countdown {
                        shot,
                        ticks_left: ticks_left - 1,
                    };
                }
            }
            CinematicPhase::Playing {
                shot,
                elapsed_ticks,
            } => {
                let elapsed = elapsed_ticks + 1;
                let duration = script::shot(shot)
                    .map(|s| secs_to_ticks(s.duration_seconds()))
                    .unwrap_or(1);
                if elapsed >= duration {
                    let next = (shot + 1 < SHOTS.len()).then_some(shot + 1);
                    self.cinematic.phase = CinematicPhase::Intermission {
                        next_shot: next,
                        ticks_left: secs_to_ticks(INTERMISSION_SECONDS),
                    };
                    envelopes.push(cue(CinematicCue::Intermission {
                        next_shot_index: next.map(|n| n as u8),
                        seconds: INTERMISSION_SECONDS,
                    }));
                } else {
                    self.cinematic.phase = CinematicPhase::Playing {
                        shot,
                        elapsed_ticks: elapsed,
                    };
                }
            }
            CinematicPhase::Intermission {
                next_shot,
                ticks_left,
            } => {
                if ticks_left <= 1 {
                    match next_shot {
                        Some(shot) => envelopes.extend(self.enter_cinematic_countdown(shot)),
                        None => envelopes.extend(self.stop_cinematic(true)),
                    }
                } else {
                    self.cinematic.phase = CinematicPhase::Intermission {
                        next_shot,
                        ticks_left: ticks_left - 1,
                    };
                }
            }
        }
        envelopes
    }

    /// Warp every real, live player who hasn't converged on the stage anchor
    /// yet. Movement is client-authoritative, so a warp only sticks once the
    /// client applies the `Correction`; until their echoed position lands
    /// near the anchor, keep re-stamping (the message can drop, see the
    /// call sites). Controls are frozen client-side during playback, so a
    /// converged player stays put.
    fn converge_cinematic_warp(&mut self) -> Vec<ServerEnvelope> {
        const CONVERGE_RADIUS_M: f32 = 25.0;
        let anchor = Vec3Net::new(layout::PLAYER_ANCHOR.x, 0.0, layout::PLAYER_ANCHOR.y);
        let stragglers: Vec<ClientId> = self
            .clients
            .values()
            .filter(|client| client.online && !client.synthetic && client.lifecycle.is_alive())
            .filter(|client| {
                !client
                    .controller
                    .position
                    .within_horizontal_range(anchor, CONVERGE_RADIUS_M)
            })
            .map(|client| client.client_id)
            .collect();
        let mut envelopes = Vec::new();
        for client_id in stragglers {
            envelopes.extend(self.warp_client(client_id, anchor, 0.0));
        }
        envelopes
    }

    /// Force-move a real client and ship the position correction their
    /// prediction needs to accept it, the same shape the combat teleport
    /// paths use.
    fn warp_client(
        &mut self,
        client_id: ClientId,
        position: Vec3Net,
        yaw: f32,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        client.controller.position = position;
        client.controller.velocity = Vec3Net::ZERO;
        client.controller.yaw = yaw;
        client.controller.grounded = true;
        let state = PlayerState {
            client_id,
            position,
            velocity: Vec3Net::ZERO,
            yaw,
            pitch: client.controller.pitch,
            health: client.controller.health,
            grounded: true,
            last_processed_input: client.controller.last_processed_input,
        };
        self.chunk_manager.update_player_chunk(client_id, position);
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Correction(state),
        }]
    }

    fn enter_cinematic_countdown(&mut self, shot_index: usize) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        if let Some(shot) = script::shot(shot_index) {
            // Lock the shot's lighting before the slate so the scene is
            // settled when the shot starts (the multiplier is already 0).
            self.set_world_time_seconds(shot.world_time_hours * 3600.0);
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            });
        }
        self.cinematic.phase = CinematicPhase::Countdown {
            shot: shot_index,
            ticks_left: secs_to_ticks(COUNTDOWN_SECONDS),
        };
        envelopes.push(cue(CinematicCue::Countdown {
            shot_index: shot_index as u8,
            seconds: COUNTDOWN_SECONDS,
        }));
        envelopes
    }

    // -----------------------------------------------------------------
    // Init phase: cleanup + stage construction
    // -----------------------------------------------------------------

    /// Strip everything transient or player-placed (dropped items, loot
    /// bags, projectiles, every deployable except the world-authored ruin
    /// caches) so the stage rebuild starts from a clean, repeatable world.
    fn cinematic_cleanup_world(&mut self) {
        let dropped: Vec<_> = self.dropped_items.keys().copied().collect();
        for id in dropped {
            if let Some(body) = self.remove_dropped_item(id) {
                self.dropped_item_physics.remove_body(body.body_handle);
                self.chunk_manager.untrack_dropped_item(id);
            }
        }
        let bags: Vec<_> = self.loot_bags.keys().copied().collect();
        for id in bags {
            self.destroy_loot_bag(id);
        }
        let projectiles: Vec<_> = self.projectiles.keys().copied().collect();
        for id in projectiles {
            self.remove_projectile(id);
        }
        let stuck: Vec<_> = self.stuck_projectiles.keys().copied().collect();
        for id in stuck {
            self.remove_projectile(id);
        }
        let deployables: Vec<_> = self
            .deployed_entities
            .iter()
            .filter(|(_, entity)| !matches!(entity.kind, DeployableKind::RuinCache))
            .map(|(id, _)| *id)
            .collect();
        for id in deployables {
            self.remove_deployed_entity_tracked(id);
        }
        self.refresh_structural_stability();
        self.recompute_claim_footprints();
    }

    fn alloc_deployed_entity_id(&mut self) -> DeployedEntityId {
        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id.0 = self.next_deployed_entity_id.0.saturating_add(1);
        id
    }

    /// Build the pre-authored base compound and props from the shared
    /// layout: ownerless world furniture, exactly like ruin caches.
    fn spawn_cinematic_stage(&mut self) {
        use crate::building::building_item_id;

        // Building blocks first (walls need their platforms for stability).
        let mut doorways: Vec<(DeployedEntityId, Vec3Net, f32)> = Vec::new();
        for block in layout::base_building_blocks() {
            let (position, yaw) = stage_block_pose(&block);
            let entity = DeployedEntity::new(
                intern_item_id(building_item_id(block.piece)),
                DeployableKind::Building {
                    piece: block.piece,
                    tier: block.tier,
                },
                position,
                yaw,
                crate::building::building_max_health(block.piece, block.tier),
                None,
                self.tick,
            );
            let id = self.alloc_deployed_entity_id();
            let entity = DeployedEntity { id, ..entity };
            self.insert_deployed_entity(id, entity);
            self.chunk_manager.track_deployed_entity(id, position);
            if matches!(block.piece, crate::building::BuildingPiece::Doorway) {
                doorways.push((id, position, yaw));
            }
        }

        // A closed hewn-log door in every doorway.
        for (parent, position, yaw) in doorways {
            let item_id = intern_item_id(crate::items::HEWN_LOG_DOOR_ID);
            let max_health = item_definition(&item_id)
                .and_then(|def| def.deployable)
                .map(|profile| profile.max_health)
                .unwrap_or(250);
            let entity = DeployedEntity {
                door: Some(super::door::DoorState {
                    code: "0000".to_owned(),
                    authorized: Vec::new(),
                    open: false,
                    parent,
                }),
                ..DeployedEntity::new(
                    item_id,
                    DeployableKind::Door {
                        variant: DoorVariant::HewnLog,
                    },
                    position,
                    yaw,
                    max_health,
                    None,
                    self.tick,
                )
            };
            let id = self.alloc_deployed_entity_id();
            let entity = DeployedEntity { id, ..entity };
            self.insert_deployed_entity(id, entity);
            self.chunk_manager.track_deployed_entity(id, position);
        }

        for prop in layout::STAGE_PROPS {
            let entity = self.build_stage_prop(prop);
            let position = entity.position;
            let id = self.alloc_deployed_entity_id();
            let entity = DeployedEntity { id, ..entity };
            self.insert_deployed_entity(id, entity);
            self.chunk_manager.track_deployed_entity(id, position);
        }

        self.refresh_structural_stability();
        self.recompute_claim_footprints();
    }

    fn build_stage_prop(&self, prop: &layout::StageProp) -> DeployedEntity {
        let (item_id, kind) = match prop.kind {
            StagePropKind::WorkbenchT1 => (
                crate::items::WORKBENCH_T1_ID,
                DeployableKind::Workbench { tier: 1 },
            ),
            StagePropKind::Furnace => (
                crate::items::CRUDE_FURNACE_ID,
                DeployableKind::Furnace { tier: 1 },
            ),
            StagePropKind::ToolCupboard => {
                (crate::items::TOOL_CUPBOARD_ID, DeployableKind::ToolCupboard)
            }
            StagePropKind::StorageBoxSmall => (
                crate::items::STORAGE_BOX_SMALL_ID,
                DeployableKind::StorageBox { tier: 1 },
            ),
            StagePropKind::SleepingBag => {
                (crate::items::SLEEPING_BAG_ID, DeployableKind::SleepingBag)
            }
            StagePropKind::TorchGround => (
                crate::items::TORCH_ID,
                DeployableKind::Torch { wall: false },
            ),
            StagePropKind::TorchWall => {
                (crate::items::TORCH_ID, DeployableKind::Torch { wall: true })
            }
        };
        let item_id = intern_item_id(item_id);
        let max_health = item_definition(&item_id)
            .and_then(|def| def.deployable)
            .map(|profile| profile.max_health)
            .unwrap_or(100);
        let mut entity = DeployedEntity::new(
            item_id,
            kind,
            Vec3Net::new(prop.x, prop.y, prop.z),
            prop.yaw,
            max_health,
            None,
            self.tick,
        );
        match prop.kind {
            StagePropKind::Furnace => {
                // Lit with a deep fuel stack and ore mid-smelt, so the shots
                // catch flame, glow, and chimney smoke the whole session.
                let mut items: [Option<ItemStack>; crate::protocol::FURNACE_ITEM_SLOT_COUNT] =
                    Default::default();
                items[0] = Some(ItemStack::new(crate::items::IRON_ORE_ID, 200));
                entity.furnace = Some(super::furnace::FurnaceState {
                    fuel: Some(ItemStack::new(crate::items::WOOD_ID, 500)),
                    items,
                    active: true,
                    ..Default::default()
                });
            }
            StagePropKind::ToolCupboard => {
                entity.cupboard = Some(super::claim::CupboardState::default());
                let mut storage = super::storage_box::StorageBoxState::new_tool_cupboard();
                // Stock upkeep generously so the base cannot decay mid-take.
                let stock = [
                    crate::items::WOOD_ID,
                    crate::items::HEWN_LOG_ID,
                    crate::items::STONE_ID,
                ];
                for (slot, item) in storage.slots.iter_mut().zip(stock) {
                    *slot = Some(ItemStack::new(item, 1000));
                }
                entity.storage = Some(storage);
            }
            StagePropKind::StorageBoxSmall => {
                entity.storage = Some(super::storage_box::StorageBoxState::new(1));
            }
            StagePropKind::TorchGround | StagePropKind::TorchWall => {
                entity.torch = Some(super::torch::TorchState::new());
            }
            _ => {}
        }
        entity
    }

    // -----------------------------------------------------------------
    // Dummy actors
    // -----------------------------------------------------------------

    fn spawn_cinematic_actors(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        for (index, spec) in layout::STAGE_ACTORS.iter().enumerate() {
            let client_id = self.next_client_id;
            self.next_client_id.0 += 1;
            let account_id = AccountId(ACTOR_ACCOUNT_BASE + index as u64);

            let mut inventory = PlayerInventoryState::empty();
            inventory.actionbar_slots[0] = Some(ItemStack::new(spec.held_item, 1));
            // The builder also carries the building plan: held during
            // placement steps, swapped back to the hammer for upgrades.
            if matches!(spec.role, ActorRole::Builder { .. }) {
                inventory.actionbar_slots[1] =
                    Some(ItemStack::new(crate::items::BUILDING_PLAN_ID, 1));
            }
            inventory.active_actionbar_slot = 0;
            for (slot_index, piece) in spec.armor.iter().enumerate() {
                if let Some(piece) = piece {
                    inventory.equipment_slots[slot_index] = Some(ItemStack::new(*piece, 1));
                }
            }
            let protection = equipped_protection(&inventory.equipment_slots);

            let mut controller = crate::controller::PlayerController::spawn();
            controller.position = Vec3Net::new(spec.spawn.x, 0.0, spec.spawn.y);
            controller.yaw = spec.yaw;

            let client = ServerClient {
                client_id,
                account_id,
                name: spec.name.to_owned(),
                online: true,
                synthetic: true,
                controller,
                inventory,
                protection,
                lifecycle: PlayerLifecycle::Alive,
                is_admin: false,
                run_speed_multiplier: 1.0,
                last_seen_tick: self.tick,
                next_gather_tick: self.tick,
                next_attack_tick: self.tick,
                draw_started_tick: None,
                use_started_tick: None,
                heal_over_time: None,
                next_ranged_tick: self.tick,
                reload_slow_active: false,
                chat_bubble: None,
                view_tier: crate::protocol::ViewRadiusTier::default(),
                crafting: super::crafting::starting_crafting_state(),
                next_craft_job_id: crate::protocol::CraftingJobId(1),
                open_furnace: None,
                open_workbench: None,
                open_container: None,
                applied_action_seq: 0,
                ping_ms: 30 + (index as u16 * 11) % 40,
                swing_seq: 0,
                swing_model: ItemModel::Bag,
            };
            let position = client.controller.position;
            self.clients.insert(client_id, client);
            self.account_to_client.insert(account_id, client_id);
            self.chunk_manager.track_player(client_id, position);
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::PlayerEvent(PlayerEvent::Joined {
                    client_id,
                    name: spec.name.to_owned(),
                }),
            });

            self.cinematic.actors.push(ActorRuntime {
                client_id,
                spec,
                // Stagger work cadences so the stage never swings in unison.
                next_swing_tick: self.tick + 10 + (index as u64) * 7,
                route_index: 0,
                work_switch_tick: self.tick + secs_to_ticks(14.0) as u64,
                dead: false,
                fight_move: FightMove::default(),
                fight_until: self.tick,
                circle_dir: if index.is_multiple_of(2) { 1.0 } else { -1.0 },
                vertical_velocity: 0.0,
            });
        }
        envelopes
    }

    /// One tick of actor choreography. Writes pose (position, velocity,
    /// yaw, and airborne hops), lands REAL gather swings on live nodes (chip
    /// VFX, node drain, trees felled through the ordinary depletion path),
    /// steps the homestead build-out, keeps the stale-sweep clock fresh, and
    /// lands the scripted skirmish kill.
    fn drive_cinematic_actors(&mut self) -> Vec<ServerEnvelope> {
        let dt = 1.0 / SERVER_TICK_RATE_HZ;
        let tick = self.tick;
        let (skirmish_elapsed, homestead_elapsed) = match self.cinematic.phase {
            CinematicPhase::Playing {
                shot,
                elapsed_ticks,
            } if shot == SKIRMISH_SHOT_INDEX => (Some(elapsed_ticks), None),
            CinematicPhase::Playing {
                shot,
                elapsed_ticks,
            } if shot == HOMESTEAD_SHOT_INDEX => (None, Some(elapsed_ticks)),
            _ => (None, None),
        };
        // Real gathers (which DRAIN nodes) only run while the harvest shot
        // plays, so the composed grove is intact when the camera arrives;
        // every other phase the workers swing for texture without draining.
        // During any countdown they stand poised instead of mid-swing.
        let harvest_live = matches!(
            self.cinematic.phase,
            CinematicPhase::Playing { shot, .. } if shot == HARVEST_SHOT_INDEX
        );
        let countdown_running = matches!(self.cinematic.phase, CinematicPhase::Countdown { .. });

        // Snapshot fighter positions for the face-your-opponent rule before
        // taking any mutable borrows.
        let fighter_positions: Vec<(ClientId, Vec3Net, bool)> = self
            .cinematic
            .actors
            .iter()
            .filter(|actor| matches!(actor.spec.role, ActorRole::Fighter { .. }))
            .map(|actor| {
                let position = self
                    .clients
                    .get(&actor.client_id)
                    .map(|c| c.controller.position)
                    .unwrap_or(Vec3Net::ZERO);
                (actor.client_id, position, actor.dead)
            })
            .collect();

        // Resolve each worker's live node target up front (immutable pass):
        // the nearest live tree / ore around the role's anchor, so a felled
        // hero pine naturally hands the chopper the next tree over.
        let work_targets: Vec<Option<(crate::protocol::ResourceNodeId, Vec3Net)>> = self
            .cinematic
            .actors
            .iter()
            .map(|actor| match actor.spec.role {
                ActorRole::Chopper { node } => self.live_node_near(node, 9.0, WorkKind::Tree),
                ActorRole::Miner { nodes } => {
                    let anchor = nodes[actor.route_index % nodes.len()];
                    self.live_node_near(anchor, 7.0, WorkKind::Ore)
                }
                _ => None,
            })
            .collect();

        let mut envelopes = Vec::new();

        // Homestead build-out: pieces appear under the builder's hammer at
        // the scripted beats; between beats the builder walks to the next
        // step's spot.
        let build_sequence = layout::homestead_build_sequence();
        let mut builder_target: Option<(Vec2, bool)> = None;
        if let Some(elapsed) = homestead_elapsed
            && let Some(timed) = build_sequence.get(self.cinematic.build_step)
        {
            {
                let hold_plan = !matches!(timed.step, layout::BuildStep::Upgrade { .. });
                builder_target = Some((build_step_position(&timed.step), hold_plan));
                if elapsed >= secs_to_ticks(timed.at_seconds) {
                    self.execute_build_step(&timed.step);
                    self.cinematic.build_step += 1;
                    let builder = self
                        .cinematic
                        .actors
                        .iter()
                        .find(|actor| matches!(actor.spec.role, ActorRole::Builder { .. }))
                        .map(|actor| actor.client_id);
                    if let Some(builder_id) = builder
                        && let Some(client) = self.clients.get_mut(&builder_id)
                    {
                        actor_swing(client);
                    }
                }
            }
        }

        // Building platform tops (foundations, ceilings) as flat AABBs, so
        // scripted movement steps UP onto them like a real controller would
        // instead of clipping through a freshly placed foundation.
        let platforms: Vec<(f32, f32, f32, f32, f32)> = self
            .deployed_entities
            .values()
            .filter_map(|entity| {
                let DeployableKind::Building { piece, .. } = entity.kind else {
                    return None;
                };
                let top = crate::building::platform_top_offset(piece)?;
                let half = crate::building::FOUNDATION_SIZE_M / 2.0;
                Some((
                    entity.position.x - half,
                    entity.position.x + half,
                    entity.position.z - half,
                    entity.position.z + half,
                    entity.position.y + top,
                ))
            })
            .collect();

        let mut pending_kill: Option<(ClientId, ClientId)> = None;
        let mut pending_gathers: Vec<(ClientId, crate::protocol::ResourceNodeId)> = Vec::new();
        let mut actors = std::mem::take(&mut self.cinematic.actors);
        for (index, actor) in actors.iter_mut().enumerate() {
            if actor.dead {
                continue;
            }
            let Some(client) = self.clients.get_mut(&actor.client_id) else {
                continue;
            };
            client.last_seen_tick = tick;

            match actor.spec.role {
                ActorRole::Chopper { .. } | ActorRole::Miner { .. } => {
                    if matches!(actor.spec.role, ActorRole::Miner { .. })
                        && tick >= actor.work_switch_tick
                    {
                        actor.route_index += 1;
                        actor.work_switch_tick = tick + secs_to_ticks(14.0) as u64;
                    }
                    let cadence = if matches!(actor.spec.role, ActorRole::Chopper { .. }) {
                        secs_to_ticks(1.3)
                    } else {
                        secs_to_ticks(1.4)
                    } as u64;
                    let cadence = if harvest_live {
                        cadence
                    } else {
                        secs_to_ticks(3.8) as u64
                    };
                    match work_targets.get(index).copied().flatten() {
                        Some((node_id, node_position)) => {
                            let spot = Vec2::new(node_position.x, node_position.z);
                            if countdown_running {
                                // Poised: hold position facing the node.
                                client.controller.velocity = Vec3Net::ZERO;
                                client.controller.yaw = face_yaw(
                                    client.controller.position,
                                    node_position.x,
                                    node_position.z,
                                );
                            } else if work_at(client, actor, tick, spot, cadence) && harvest_live {
                                pending_gathers.push((actor.client_id, node_id));
                            }
                        }
                        // Nothing left to work nearby: stand easy.
                        None => {
                            client.controller.velocity = Vec3Net::ZERO;
                        }
                    }
                }
                ActorRole::Builder { target } => {
                    match builder_target {
                        Some((piece, hold_plan)) => {
                            // Real builders hold the plan to place and the
                            // hammer to upgrade; mirror that.
                            client.inventory.active_actionbar_slot = if hold_plan { 1 } else { 0 };
                            // Stand OFF the piece (outward from the base
                            // centre) so the builder never stands inside a
                            // foundation as it appears under them.
                            let away = (piece - layout::BASE_CENTER).normalize_or_zero();
                            let stand = piece + away * 2.4;
                            walk_towards(client, stand, ACTOR_WALK_SPEED, dt, 0.5);
                            client.controller.yaw =
                                face_yaw(client.controller.position, piece.x, piece.y);
                        }
                        None => {
                            client.inventory.active_actionbar_slot = 0;
                            // Ambient: hammer taps on the cabin's east wall.
                            work_at(client, actor, tick, target, secs_to_ticks(3.2) as u64);
                        }
                    }
                }
                ActorRole::Wanderer { waypoints } => {
                    if waypoints.is_empty() {
                        continue;
                    }
                    let target = waypoints[actor.route_index % waypoints.len()];
                    if walk_towards(client, target, ACTOR_WALK_SPEED, dt, 0.7) {
                        actor.route_index = (actor.route_index + 1) % waypoints.len();
                    }
                }
                ActorRole::Fighter { arena, dies } => {
                    let opponent = fighter_positions
                        .iter()
                        .find(|(id, _, _)| *id != actor.client_id)
                        .copied();
                    let fighting = skirmish_elapsed.is_some();
                    drive_fighter(client, actor, tick, dt, arena, opponent, fighting);
                    if dies
                        && let Some(elapsed) = skirmish_elapsed
                        && elapsed == secs_to_ticks(SKIRMISH_KILL_SECONDS)
                        && let Some((killer_id, _, _)) = opponent
                    {
                        pending_kill = Some((actor.client_id, killer_id));
                        actor.dead = true;
                    }
                }
            }

            // Ground the actor on whatever platform is underfoot (fighters
            // manage their own hop y in `drive_fighter`; the arena has no
            // platforms, so this is a no-op for them).
            if !matches!(actor.spec.role, ActorRole::Fighter { .. }) {
                let position = client.controller.position;
                let surface = platform_surface_y(&platforms, position.x, position.z);
                if (position.y - surface).abs() > 0.001 {
                    client.controller.position = Vec3Net::new(position.x, surface, position.z);
                }
            }
        }
        self.cinematic.actors = actors;

        // Real gathers: chips fly, storage drains, and a drained tree falls
        // through the ordinary depletion broadcast.
        for (actor_id, node_id) in pending_gathers {
            envelopes.extend(self.actor_gather(actor_id, node_id));
        }
        if let Some((victim, killer)) = pending_kill {
            // Actor names are 'static, so this resolves to a plain str with
            // no borrow held across the kill call.
            let killer_name: &'static str = self
                .cinematic
                .actors
                .iter()
                .find(|runtime| runtime.client_id == killer)
                .map(|runtime| runtime.spec.name)
                .unwrap_or("Raider");
            envelopes.extend(self.kill_player(victim, Some(killer), killer_name));
        }
        envelopes
    }

    /// Nearest live resource node of `kind` within `radius` of `center`.
    fn live_node_near(
        &self,
        center: Vec2,
        radius: f32,
        kind: WorkKind,
    ) -> Option<(crate::protocol::ResourceNodeId, Vec3Net)> {
        let radius_sq = radius * radius;
        self.resource_nodes
            .values()
            .filter(|node| !node.dead)
            .filter(|node| {
                crate::resource_nodes::resource_node_definition(&node.definition_id).is_some_and(
                    |definition| match kind {
                        WorkKind::Tree => definition.model.is_tree(),
                        WorkKind::Ore => {
                            !definition.model.is_tree() && !definition.model.is_crude()
                        }
                    },
                )
            })
            .filter_map(|node| {
                let dx = node.position.x - center.x;
                let dz = node.position.z - center.y;
                let d2 = dx * dx + dz * dz;
                (d2 <= radius_sq).then_some((d2.to_bits(), node.id, node.position))
            })
            .min_by_key(|(d2, _, _)| *d2)
            .map(|(_, id, position)| (id, position))
    }

    /// Land one REAL gather swing on `node_id` as `actor_id`: aim the
    /// controller at the node's anchor (the gather path re-validates the view
    /// ray), open the cooldown, run the ordinary gather (payout, chip VFX
    /// broadcast, depletion), and refresh the tool so stage tools never wear
    /// out mid-take.
    fn actor_gather(
        &mut self,
        actor_id: ClientId,
        node_id: crate::protocol::ResourceNodeId,
    ) -> Vec<ServerEnvelope> {
        let Some(node) = self.resource_nodes.get(&node_id) else {
            return Vec::new();
        };
        let anchor = crate::resource_nodes::resource_node_anchor(node);
        if let Some(client) = self.clients.get_mut(&actor_id) {
            let eye = super::movement::player_eye_position(client.controller.position);
            let dx = anchor.x - eye.x;
            let dy = anchor.y - eye.y;
            let dz = anchor.z - eye.z;
            client.controller.yaw = (-dx).atan2(-dz);
            client.controller.pitch = dy.atan2((dx * dx + dz * dz).sqrt());
            client.next_gather_tick = client.next_gather_tick.min(self.tick);
        }
        let mut envelopes = self.apply_gather_command(
            actor_id,
            crate::protocol::ResourceGatherCommand {
                resource_node_id: node_id,
                seq: 0,
                hit_point: anchor,
            },
        );
        if let Some(client) = self.clients.get_mut(&actor_id)
            && let Some(stack) = client
                .inventory
                .actionbar_slots
                .first_mut()
                .and_then(|slot| slot.as_mut())
            && let Some(definition) = item_definition(&stack.item_id)
            && let Some(tool) = definition.tool
        {
            stack.durability = tool.max_durability;
        }
        // Actor-bound envelopes (full-inventory toasts) have no transport;
        // drop them rather than route no-ops.
        envelopes.retain(
            |envelope| !matches!(envelope.target, DeliveryTarget::Client(id) if id == actor_id),
        );
        envelopes
    }

    /// Cap a node's remaining storage so it breaks after exactly `swings`
    /// more real gathers with `tool_item`. Called at the harvest countdown so
    /// the hero pine falls (and the ore breaks) mid-shot, on camera.
    fn pre_drain_node_for_swings(&mut self, near: Vec2, tool_item: &str, swings: u32) {
        let Some(tool) = item_definition(&intern_item_id(tool_item)).and_then(|d| d.tool) else {
            return;
        };
        let Some((node_id, _)) = self
            .live_node_near(near, 3.0, WorkKind::Tree)
            .or_else(|| self.live_node_near(near, 3.0, WorkKind::Ore))
        else {
            return;
        };
        let Some(payout) = self
            .resource_nodes
            .get(&node_id)
            .and_then(|node| crate::resource_nodes::next_resource_payout(node, tool))
        else {
            return;
        };
        let total = (u32::from(payout.quantity) * swings).min(u32::from(u16::MAX)) as u16;
        if let Some(node) = self.resource_nodes.get_mut(&node_id) {
            node.storage = vec![ItemStack::new(payout.item_id.as_ref(), total)];
        }
    }

    /// Reset the authored stage nodes to their fresh, alive state: replays
    /// must find the exact same composed grove regardless of what earlier
    /// takes felled or drained. Missing nodes respawn; drained ones refill.
    fn restore_stage_nodes(&mut self) {
        for (definition_id, x, z, yaw) in layout::authored_node_placements() {
            let existing = self
                .resource_nodes
                .values()
                .find(|node| {
                    node.definition_id == *definition_id
                        && (node.position.x - x).abs() < 0.25
                        && (node.position.z - z).abs() < 0.25
                })
                .map(|node| node.id);
            match existing {
                Some(id) => {
                    if let Some(definition) =
                        crate::resource_nodes::resource_node_definition(definition_id)
                        && let Some(node) = self.resource_nodes.get_mut(&id)
                    {
                        node.storage = crate::resource_nodes::definition_storage_stacks(definition);
                        node.dead = false;
                    }
                }
                None => {
                    let id = self.allocate_resource_node_id();
                    let spawn = crate::world::WorldResourceNodeSpawn::new(
                        id,
                        *definition_id,
                        Vec3Net::new(*x, 0.0, *z),
                        *yaw,
                    );
                    let Some(node) = crate::resource_nodes::spawn_resource_node(&spawn, None)
                    else {
                        continue;
                    };
                    let Some(kind) = crate::world::NodeKind::from_definition_id(definition_id)
                    else {
                        continue;
                    };
                    self.chunk_manager
                        .track_resource_node(id, kind, node.position);
                    self.insert_resource_node(id, node);
                }
            }
        }
    }

    /// Apply one homestead build step: a block appears, a block upgrades in
    /// place (model + HP follow the tier), or a prop goes down.
    fn execute_build_step(&mut self, step: &layout::BuildStep) {
        match step {
            layout::BuildStep::Block(block) => {
                let (position, yaw) = stage_block_pose(block);
                let entity = DeployedEntity::new(
                    intern_item_id(crate::building::building_item_id(block.piece)),
                    DeployableKind::Building {
                        piece: block.piece,
                        tier: block.tier,
                    },
                    position,
                    yaw,
                    crate::building::building_max_health(block.piece, block.tier),
                    None,
                    self.tick,
                );
                let id = self.alloc_deployed_entity_id();
                let entity = DeployedEntity { id, ..entity };
                self.insert_deployed_entity(id, entity);
                self.chunk_manager.track_deployed_entity(id, position);
                self.refresh_structural_stability();
            }
            layout::BuildStep::Upgrade { cell, edge, tier } => {
                let probe = layout::StageBuildingBlock {
                    piece: crate::building::BuildingPiece::Wall,
                    tier: *tier,
                    cell: *cell,
                    edge: *edge,
                    level: 0,
                };
                let (position, _) = stage_block_pose(&probe);
                let target = self
                    .deployed_entities
                    .iter()
                    .find(|(_, entity)| {
                        matches!(entity.kind, DeployableKind::Building { .. })
                            && (entity.position.x - position.x).abs() < 0.1
                            && (entity.position.y - position.y).abs() < 0.1
                            && (entity.position.z - position.z).abs() < 0.1
                    })
                    .map(|(id, _)| *id);
                if let Some(id) = target
                    && let Some(entity) = self.deployed_entity_mut(id)
                    && let DeployableKind::Building { piece, .. } = entity.kind
                {
                    entity.kind = DeployableKind::Building { piece, tier: *tier };
                    entity.max_health = crate::building::building_max_health(piece, *tier);
                    entity.health = entity.max_health;
                }
            }
            layout::BuildStep::Prop(prop) => {
                let entity = self.build_stage_prop(prop);
                let position = entity.position;
                let id = self.alloc_deployed_entity_id();
                let entity = DeployedEntity { id, ..entity };
                self.insert_deployed_entity(id, entity);
                self.chunk_manager.track_deployed_entity(id, position);
            }
        }
    }
}

/// Walkable surface height at `(x, z)`: the highest building platform top
/// containing the point, or the ground plane.
fn platform_surface_y(platforms: &[(f32, f32, f32, f32, f32)], x: f32, z: f32) -> f32 {
    platforms
        .iter()
        .filter(|(min_x, max_x, min_z, max_z, _)| {
            x >= *min_x && x <= *max_x && z >= *min_z && z <= *max_z
        })
        .map(|(_, _, _, _, top)| *top)
        .fold(0.0, f32::max)
}

/// Which family of node a worker actor targets.
#[derive(Debug, Clone, Copy)]
enum WorkKind {
    Tree,
    Ore,
}

/// World XZ a build step lands at (where the builder walks to).
fn build_step_position(step: &layout::BuildStep) -> Vec2 {
    match step {
        layout::BuildStep::Block(block) => {
            let (position, _) = stage_block_pose(block);
            Vec2::new(position.x, position.z)
        }
        layout::BuildStep::Upgrade { cell, edge, tier } => {
            let probe = layout::StageBuildingBlock {
                piece: crate::building::BuildingPiece::Wall,
                tier: *tier,
                cell: *cell,
                edge: *edge,
                level: 0,
            };
            let (position, _) = stage_block_pose(&probe);
            Vec2::new(position.x, position.z)
        }
        layout::BuildStep::Prop(prop) => Vec2::new(prop.x, prop.z),
    }
}

/// One tick of fighter choreography. Continuous velocity (no start/stop
/// stutter on the walk animator), an aggressive fight loop while the
/// skirmish shot rolls (lunge in, strike, hop back out, flip circling
/// direction), squared-up sparring otherwise, and real vertical hops
/// (position.y + grounded replicate, so peers see the jump).
fn drive_fighter(
    client: &mut ServerClient,
    actor: &mut ActorRuntime,
    tick: u64,
    dt: f32,
    arena: Vec2,
    opponent: Option<(ClientId, Vec3Net, bool)>,
    fighting: bool,
) {
    let position = client.controller.position;
    let opponent_position = opponent
        .map(|(_, p, _)| Vec2::new(p.x, p.z))
        .unwrap_or(arena);
    let opponent_dead = opponent.is_some_and(|(_, _, dead)| dead);
    let to_opp = opponent_position - Vec2::new(position.x, position.z);
    let dist = to_opp.length().max(0.01);
    let dir = to_opp / dist;
    let tangent = Vec2::new(-dir.y, dir.x) * actor.circle_dir;

    let roll = |salt: u64| -> f32 {
        (crate::world::splitmix64(tick ^ actor.client_id.0.wrapping_mul(0x9E37) ^ salt) >> 40)
            as f32
            / (1u64 << 24) as f32
    };

    // State transitions.
    if tick >= actor.fight_until && !opponent_dead {
        actor.fight_move = if fighting {
            let r = roll(1);
            if matches!(actor.fight_move, FightMove::Lunge) {
                // A lunge always breaks off (hop back out or slide away).
                actor.circle_dir = if roll(2) < 0.5 { 1.0 } else { -1.0 };
                actor.fight_until = tick + secs_to_ticks(0.5 + roll(3) * 0.4) as u64;
                if roll(4) < 0.65 && client.controller.position.y <= 0.01 {
                    actor.vertical_velocity = 4.6;
                }
                FightMove::Retreat
            } else if r < 0.55 {
                actor.fight_until = tick + secs_to_ticks(1.2) as u64;
                FightMove::Lunge
            } else {
                actor.circle_dir = -actor.circle_dir;
                actor.fight_until = tick + secs_to_ticks(0.6 + roll(5) * 0.8) as u64;
                if roll(6) < 0.25 && client.controller.position.y <= 0.01 {
                    actor.vertical_velocity = 4.2;
                }
                FightMove::Circle
            }
        } else {
            if roll(7) < 0.3 {
                actor.circle_dir = -actor.circle_dir;
            }
            actor.fight_until = tick + secs_to_ticks(1.6 + roll(8) * 1.2) as u64;
            FightMove::Circle
        };
    }

    // Velocity from the current move: always continuous, never a dead stop.
    let desired_radius = if fighting { 2.5 } else { 3.1 };
    // Keep the duel anchored to the clearing: past a few metres out, both
    // fighters get pulled back toward the arena centre so hours of orbiting
    // can never drift the fight off camera.
    let my_pos = Vec2::new(position.x, position.z);
    let from_arena = my_pos - arena;
    let arena_pull = if from_arena.length() > 4.5 {
        -from_arena.normalize_or_zero() * (from_arena.length() - 4.5).min(3.0)
    } else {
        Vec2::ZERO
    };
    let planar = if opponent_dead {
        // Stand over the fallen opponent, drifting to a halt.
        Vec2::ZERO
    } else {
        arena_pull
            + match actor.fight_move {
                FightMove::Circle => {
                    let speed = if fighting { 3.4 } else { 2.2 };
                    let radial = dir * (dist - desired_radius) * 1.6;
                    (tangent * speed + radial).clamp_length_max(speed + 1.2)
                }
                FightMove::Lunge => {
                    if dist > 1.55 {
                        dir * 6.8
                    } else {
                        // Contact: break off next transition; keep a grazing slide.
                        tangent * 2.2
                    }
                }
                FightMove::Retreat => (-dir * 4.8 + tangent * 1.4).clamp_length_max(5.2),
            }
    };

    // Vertical hop integration (a real jump: peers replicate y + grounded).
    let mut y = client.controller.position.y;
    if actor.vertical_velocity != 0.0 || y > 0.0 {
        y += actor.vertical_velocity * dt;
        actor.vertical_velocity -= 14.0 * dt;
        if y <= 0.0 {
            y = 0.0;
            actor.vertical_velocity = 0.0;
        }
    }
    client.controller.grounded = y <= 0.001;

    client.controller.position =
        Vec3Net::new(position.x + planar.x * dt, y, position.z + planar.y * dt);
    client.controller.velocity = Vec3Net::new(planar.x, actor.vertical_velocity.max(0.0), planar.y);
    if !opponent_dead {
        client.controller.yaw = face_yaw(
            client.controller.position,
            opponent_position.x,
            opponent_position.y,
        );
    }

    // Strikes: in a real fight, blows land whenever the fighters are in
    // reach, fast; sparring keeps a slow, wary cadence.
    if !opponent_dead {
        let cadence = if fighting {
            secs_to_ticks(0.5 + roll(9) * 0.3)
        } else {
            secs_to_ticks(2.4)
        } as u64;
        if dist < 2.9 && tick >= actor.next_swing_tick {
            actor_swing(client);
            actor.next_swing_tick = tick + cadence;
        }
    }
}

/// Walk `client` toward `target` at `speed`; returns true when arrived
/// (within `arrive_m`). While moving, velocity is the real step velocity so
/// the peer walk animator strides; on arrival velocity zeroes and the walk
/// cycle settles.
fn walk_towards(
    client: &mut ServerClient,
    target: Vec2,
    speed: f32,
    dt: f32,
    arrive_m: f32,
) -> bool {
    let position = client.controller.position;
    let dx = target.x - position.x;
    let dz = target.y - position.z;
    let distance = (dx * dx + dz * dz).sqrt();
    if distance <= arrive_m {
        client.controller.velocity = Vec3Net::ZERO;
        return true;
    }
    let step = (speed * dt).min(distance);
    let (nx, nz) = (dx / distance, dz / distance);
    client.controller.position = Vec3Net::new(position.x + nx * step, 0.0, position.z + nz * step);
    client.controller.velocity = Vec3Net::new(nx * speed, 0.0, nz * speed);
    client.controller.yaw = (-dx).atan2(-dz);
    client.controller.grounded = true;
    false
}

/// Shared work loop for the chopper / miner / builder roles: walk to a
/// stand-off spot near the node, face it, and swing on `cadence_ticks`.
/// Returns true when a swing fired this tick (the caller lands the real
/// gather for it).
fn work_at(
    client: &mut ServerClient,
    actor: &mut ActorRuntime,
    tick: u64,
    node: Vec2,
    cadence_ticks: u64,
) -> bool {
    let position = client.controller.position;
    let dx = node.x - position.x;
    let dz = node.y - position.z;
    let distance = (dx * dx + dz * dz).sqrt();
    if distance > WORK_STAND_OFF_M + WORK_ARRIVE_M {
        // Approach a stand-off point just short of the node, not its centre.
        let ratio = (distance - WORK_STAND_OFF_M) / distance;
        let target = Vec2::new(position.x + dx * ratio, position.z + dz * ratio);
        let dt = 1.0 / SERVER_TICK_RATE_HZ;
        walk_towards(client, target, ACTOR_WALK_SPEED, dt, WORK_ARRIVE_M);
        return false;
    }
    client.controller.velocity = Vec3Net::ZERO;
    client.controller.yaw = face_yaw(position, node.x, node.y);
    if tick >= actor.next_swing_tick {
        actor_swing(client);
        actor.next_swing_tick = tick + cadence_ticks;
        return true;
    }
    false
}

/// Bump the replicated swing state exactly like an accepted `SwingStart`:
/// peers edge-detect `swing_seq` and play the held item's swing archetype.
fn actor_swing(client: &mut ServerClient) {
    let model = client
        .inventory
        .active_actionbar_stack()
        .and_then(|stack| item_definition(&stack.item_id))
        .map(|definition| definition.swing_model())
        .unwrap_or(ItemModel::Bag);
    client.swing_seq = client.swing_seq.wrapping_add(1);
    client.swing_model = model;
}

/// Total scripted runtime, for the start toast.
fn cinematic_total_seconds() -> f32 {
    let shots: f32 = SHOTS.iter().map(|shot| shot.duration_seconds()).sum();
    INIT_SECONDS
        + shots
        + SHOTS.len() as f32 * COUNTDOWN_SECONDS
        + SHOTS.len() as f32 * INTERMISSION_SECONDS
}

/// World pose of one authored building block on the base grid. Platform
/// pieces sit at the cell centre (foundations on the ground, ceilings one
/// wall-height up); wall-like pieces stand on the platform-edge socket,
/// matching `crate::building::sockets` exactly (edge midpoint at platform
/// top, walls on the ±X edges quarter-turned).
fn stage_block_pose(block: &StageBuildingBlock) -> (Vec3Net, f32) {
    use crate::building::{
        BuildingPiece, CEILING_THICKNESS_M, FOUNDATION_HEIGHT_M, FOUNDATION_SIZE_M, WALL_HEIGHT_M,
    };
    let cell_x = layout::BASE_ORIGIN.x + block.cell.0 as f32 * FOUNDATION_SIZE_M;
    let cell_z = layout::BASE_ORIGIN.y + block.cell.1 as f32 * FOUNDATION_SIZE_M;
    let half = FOUNDATION_SIZE_M / 2.0;
    match block.edge {
        None => {
            let y = match block.piece {
                // Nested slab, matching `wall_ceiling_sockets`: a carried
                // ceiling's base sits one slab thickness below the wall top
                // so its walkable surface is flush with the wall's top edge.
                // Without this the stability pass sees no support under the
                // roof and culls it.
                BuildingPiece::Ceiling => {
                    FOUNDATION_HEIGHT_M + WALL_HEIGHT_M * block.level.max(1) as f32
                        - CEILING_THICKNESS_M
                }
                _ => 0.0,
            };
            (Vec3Net::new(cell_x, y, cell_z), 0.0)
        }
        Some(edge) => {
            let top = FOUNDATION_HEIGHT_M + WALL_HEIGHT_M * block.level as f32;
            let (dx, dz, yaw) = match edge {
                CellEdge::North => (0.0, -half, 0.0),
                CellEdge::South => (0.0, half, 0.0),
                CellEdge::East => (half, 0.0, std::f32::consts::FRAC_PI_2),
                CellEdge::West => (-half, 0.0, std::f32::consts::FRAC_PI_2),
            };
            (Vec3Net::new(cell_x + dx, top, cell_z + dz), yaw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthMode;
    use crate::protocol::ServerMessage;
    use crate::save::WorldSave;
    use crate::server::test_support::connect_host;
    use crate::server::{GameServer, ServerSettings};

    fn cinematic_server() -> GameServer {
        GameServer::new(
            WorldSave::new_with_map(
                "Cinema",
                Some(crate::protocol::AccountId(1)),
                MapType::Cinematic,
            ),
            ServerSettings {
                auth_mode: AuthMode::NoAuth,
                singleplayer_host: Some(crate::protocol::AccountId(1)),
            },
        )
    }

    fn cues(envelopes: &[ServerEnvelope]) -> Vec<CinematicCue> {
        envelopes
            .iter()
            .filter_map(|envelope| match &envelope.message {
                ServerMessage::Cinematic(cue) => Some(cue.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cinematic_requires_a_stage_world() {
        let mut server = crate::server::test_support::server();
        let host = connect_host(&mut server);
        let envelopes = server.apply_command(host, "cinematic".to_owned());
        assert!(!server.cinematic.active());
        assert!(envelopes.iter().any(|envelope| matches!(
            &envelope.message,
            ServerMessage::Toast(toast) if toast.text.contains("Cinematic Stage")
        )));
    }

    #[test]
    fn stage_world_generation_is_deterministic_with_a_clean_authored_stage() {
        let collect = |server: &GameServer| {
            let mut nodes: Vec<(u64, String, i64, i64, bool)> = server
                .resource_nodes
                .values()
                .map(|node| {
                    (
                        node.id.0,
                        node.definition_id.clone(),
                        (node.position.x * 100.0) as i64,
                        (node.position.z * 100.0) as i64,
                        node.dead,
                    )
                })
                .collect();
            nodes.sort();
            nodes
        };
        let first = collect(&cinematic_server());
        let second = collect(&cinematic_server());
        assert_eq!(first, second, "cinematic worlds must be identical");

        // The hero pine stands exactly where authored, always alive.
        let server = cinematic_server();
        let hero = server
            .resource_nodes
            .values()
            .find(|node| {
                (node.position.x - layout::GROVE_HERO_TREE.x).abs() < 0.01
                    && (node.position.z - layout::GROVE_HERO_TREE.y).abs() < 0.01
            })
            .expect("hero pine must exist");
        assert!(!hero.dead, "authored trees never roll into dead snags");

        // Inside the stage clear zones only the authored placements exist:
        // the exclusion footprints must have kept procedural scatter out.
        let authored: Vec<(i64, i64)> = layout::authored_node_placements()
            .iter()
            .map(|(_, x, z, _)| ((*x * 100.0) as i64, (*z * 100.0) as i64))
            .collect();
        for node in server.resource_nodes.values() {
            let in_zone = layout::STAGE_ZONES.iter().any(|zone| {
                let dx = node.position.x - zone.x;
                let dz = node.position.z - zone.z;
                dx * dx + dz * dz <= zone.radius * zone.radius
            });
            if in_zone {
                let key = (
                    (node.position.x * 100.0) as i64,
                    (node.position.z * 100.0) as i64,
                );
                assert!(
                    authored.contains(&key),
                    "unexpected procedural node inside a stage zone at ({}, {})",
                    node.position.x,
                    node.position.z
                );
            }
        }
    }

    #[test]
    fn playback_warps_the_issuer_to_the_stage_anchor() {
        let mut server = cinematic_server();
        let host = connect_host(&mut server);
        server
            .clients
            .get_mut(&host)
            .expect("host connected")
            .controller
            .position = Vec3Net::new(-400.0, 0.0, -300.0);

        let envelopes = server.apply_command(host, "cinematic".to_owned());

        let position = server
            .clients
            .get(&host)
            .expect("host connected")
            .controller
            .position;
        assert!(
            (position.x - layout::PLAYER_ANCHOR.x).abs() < 0.01
                && (position.z - layout::PLAYER_ANCHOR.y).abs() < 0.01,
            "issuer was not warped to the anchor: ({}, {})",
            position.x,
            position.z
        );
        assert!(
            envelopes.iter().any(|envelope| matches!(
                &envelope.message,
                ServerMessage::Correction(state)
                    if state.client_id == host
                        && (state.position.x - layout::PLAYER_ANCHOR.x).abs() < 0.01
            )),
            "no warp Correction envelope for the issuer"
        );
    }

    #[test]
    fn playback_builds_the_stage_counts_down_and_stops_clean() {
        let mut server = cinematic_server();
        let host = connect_host(&mut server);

        let envelopes = server.apply_command(host, "cinematic play".to_owned());
        assert!(matches!(
            server.cinematic.phase,
            CinematicPhase::Initializing { .. }
        ));
        assert!(cues(&envelopes).contains(&CinematicCue::Initializing));

        // Stage: every authored building block, a door for the doorway, and
        // the props (lit furnace among them).
        let blocks = layout::base_building_blocks().len();
        let buildings = server
            .deployed_entities
            .values()
            .filter(|entity| matches!(entity.kind, DeployableKind::Building { .. }))
            .count();
        assert_eq!(buildings, blocks);
        let doors = server
            .deployed_entities
            .values()
            .filter(|entity| matches!(entity.kind, DeployableKind::Door { .. }))
            .count();
        assert_eq!(doors, 1);
        assert!(server.deployed_entities.values().any(|entity| {
            matches!(entity.kind, DeployableKind::Furnace { .. })
                && entity
                    .furnace
                    .as_ref()
                    .is_some_and(|furnace| furnace.active)
        }));

        // Dummy actors joined as synthetic online players.
        let synthetic = server
            .clients
            .values()
            .filter(|client| client.synthetic)
            .count();
        assert_eq!(synthetic, layout::STAGE_ACTORS.len());
        // ... and never reach the save.
        let save = server.world_save();
        assert!(
            save.state
                .players
                .iter()
                .all(|player| player.account_id.0 < super::ACTOR_ACCOUNT_BASE),
            "synthetic actors must not persist"
        );

        // Init phase elapses into the first countdown, then the shot.
        // Accumulate the liveliness checks across the run: any individual
        // tick can legitimately catch every actor standing (work poses,
        // waypoint arrivals), but across eleven seconds someone must have
        // walked and someone must have swung.
        let mut saw_countdown = false;
        let mut saw_shot_started = false;
        let mut moved = false;
        for _ in 0..secs_to_ticks(INIT_SECONDS + COUNTDOWN_SECONDS + 2.0) {
            let envelopes = server.tick(1.0 / SERVER_TICK_RATE_HZ);
            for cue in cues(&envelopes) {
                match cue {
                    CinematicCue::Countdown { shot_index: 0, .. } => saw_countdown = true,
                    CinematicCue::ShotStarted { shot_index: 0 } => saw_shot_started = true,
                    _ => {}
                }
            }
            moved |= server.clients.values().any(|client| {
                let velocity = client.controller.velocity;
                client.synthetic && (velocity.x * velocity.x + velocity.z * velocity.z) > 0.01
            });
        }
        assert!(saw_countdown, "countdown cue for shot 0 never fired");
        assert!(saw_shot_started, "shot 0 never started");
        assert!(matches!(
            server.cinematic.phase,
            CinematicPhase::Playing { shot: 0, .. }
        ));
        let swung = server
            .clients
            .values()
            .any(|client| client.synthetic && client.swing_seq > 0);
        assert!(moved, "no actor moved during the opening shot");
        assert!(swung, "no actor has swung by mid-shot");

        // Stop tears everything down.
        let envelopes = server.apply_command(host, "cinematic stop".to_owned());
        assert!(cues(&envelopes).contains(&CinematicCue::Stopped));
        assert!(!server.cinematic.active());
        assert_eq!(
            server
                .clients
                .values()
                .filter(|client| client.synthetic)
                .count(),
            0
        );
    }
}
