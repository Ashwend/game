//! Server-side `/command` handling.
//!
//! Slash commands are typed in chat and shipped to the server as
//! `ClientMessage::Command { text }`. The server is the source of truth for
//! parsing, the admin check, and any state mutation. The client only knows
//! how to tell chat input apart from command input by the leading `/`.
//!
//! Each command yields a `Vec<ServerEnvelope>` like the rest of the receive
//! path — a Toast (back to the issuer) plus any side-effects (resource node
//! insert, broadcast snapshot pickup on the next tick, etc.).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    items::{
        BASIC_HATCHET_ID, BASIC_PICKAXE_ID, COAL_ID, CRUDE_FURNACE_ID, FIBER_ID, IRON_BAR_ID,
        IRON_ORE_ID, PLANT_TWINE_ID, STONE_ID, SULFUR_ORE_ID, WOOD_ID, WORKBENCH_T1_ID,
    },
    protocol::{
        ChatMessage, ClientId, ItemStack, ResourceNodeId, ServerMessage, ToastKind, ToastMessage,
        Vec3Net,
    },
    resources::{
        COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID, resource_node_definition, spawn_resource_node,
    },
    world::{NodeKind, WorldResourceNodeSpawn},
    world_time::{MAX_MULTIPLIER, MIN_MULTIPLIER, parse_time_token},
};

use super::{DeliveryTarget, GameServer, ServerEnvelope, inventory::add_stack_to_inventory};

/// Hard limit on `/spawn-ore` radius. Keeps an admin debug command from
/// accidentally placing a node hundreds of meters away in a flat world.
const MAX_SPAWN_ORE_RADIUS: f32 = 30.0;
const DEFAULT_SPAWN_ORE_RADIUS: f32 = 8.0;
/// Minimum distance between the spawned node and the issuer. Keeps the
/// new node from materialising inside the player's collision radius.
const MIN_SPAWN_ORE_DISTANCE: f32 = 1.75;

impl GameServer {
    /// Apply a `ClientMessage::Command` payload. Trims the leading slash if
    /// the client forgot to strip it, splits on whitespace, and dispatches
    /// to per-command handlers.
    pub(super) fn apply_command(
        &mut self,
        client_id: ClientId,
        text: String,
    ) -> Vec<ServerEnvelope> {
        let trimmed = text.trim().trim_start_matches('/');
        if trimmed.is_empty() {
            return reply_warning(client_id, "empty command");
        }

        let mut parts = trimmed.split_whitespace();
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        let args: Vec<&str> = parts.collect();

        match name.as_str() {
            "spawn-ore" | "spawnore" => self.command_spawn_ore(client_id, &args),
            "time" => self.command_set_time(client_id, &args),
            "speed" | "timescale" => self.command_set_time_multiplier(client_id, &args),
            "test-kit" | "testkit" => self.command_test_kit(client_id),
            "tp" | "teleport" => self.command_teleport_all(client_id),
            "help" => self.command_help(client_id),
            other => reply_warning(client_id, format!("unknown command: /{other}")),
        }
    }

    /// `/help` — drop the command list into the issuer's chat log as
    /// messages from "Server" (rather than a toast) so it lingers, scrolls,
    /// and reads alongside normal conversation. Only the issuer sees it.
    fn command_help(&self, client_id: ClientId) -> Vec<ServerEnvelope> {
        // Whether each line is admin-only. Non-admins still see the section
        // but the rendered list tells them what's gated, instead of leaving
        // the impression that nothing exists.
        let is_admin = self
            .clients
            .get(&client_id)
            .map(|client| client.is_admin)
            .unwrap_or(false);

        let mut lines: Vec<String> = vec!["Available commands:".to_owned()];
        lines.push("  /help: show this list".to_owned());
        let spawn_ore_line = if is_admin {
            "  /spawn-ore [coal|iron|sulfur] [radius]: drop a fresh ore node nearby"
        } else {
            "  /spawn-ore [coal|iron|sulfur] [radius]: admin only"
        };
        lines.push(spawn_ore_line.to_owned());
        let time_line = if is_admin {
            "  /time <HH:MM|hour>: set the time of day"
        } else {
            "  /time <HH:MM|hour>: admin only"
        };
        lines.push(time_line.to_owned());
        let speed_line = if is_admin {
            "  /speed <multiplier>: set the day/night speed (0 to 240)"
        } else {
            "  /speed <multiplier>: admin only"
        };
        lines.push(speed_line.to_owned());
        let test_kit_line = if is_admin {
            "  /test-kit: grant every tool + 100 of each resource + 1 workbench + 1 furnace"
        } else {
            "  /test-kit: admin only"
        };
        lines.push(test_kit_line.to_owned());
        let tp_line = if is_admin {
            "  /tp: teleport every other connected player to your position (for PvP/death testing)"
        } else {
            "  /tp: admin only"
        };
        lines.push(tp_line.to_owned());

        lines
            .into_iter()
            .map(|text| ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Chat(ChatMessage {
                    from: "Server".to_owned(),
                    text,
                }),
            })
            .collect()
    }

    fn command_set_time(&mut self, client_id: ClientId, args: &[&str]) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        let Some(token) = args.first() else {
            return reply_warning(client_id, "usage: /time <HH:MM> or /time <hour>");
        };
        let Some(seconds) = parse_time_token(token) else {
            return reply_warning(
                client_id,
                format!("could not parse '{token}'; try '/time 06:30' or '/time 14'"),
            );
        };

        self.set_world_time_seconds(seconds);
        let label = self.world_time.format_hhmm();
        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            },
            ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Toast(ToastMessage::new(
                    ToastKind::Success,
                    format!("time set to {label}"),
                )),
            },
        ]
    }

    fn command_set_time_multiplier(
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

        let Some(token) = args.first() else {
            return reply_warning(
                client_id,
                format!("usage: /speed <multiplier> (0 to {MAX_MULTIPLIER})"),
            );
        };
        let Ok(multiplier) = token.parse::<f32>() else {
            return reply_warning(client_id, format!("could not parse '{token}' as a number"));
        };
        if !multiplier.is_finite() || multiplier < MIN_MULTIPLIER {
            return reply_warning(
                client_id,
                format!("multiplier must be in [{MIN_MULTIPLIER}, {MAX_MULTIPLIER}]"),
            );
        }

        self.set_world_time_multiplier(multiplier);
        let applied = self.world_time.multiplier;
        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            },
            ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Toast(ToastMessage::new(
                    ToastKind::Success,
                    format!("day/night speed set to {applied:.2}×"),
                )),
            },
        ]
    }

    /// `/spawn-ore [coal|iron|sulfur] [radius]`
    ///
    /// Picks a random horizontal offset within `radius` of the issuing
    /// player and inserts a fresh node at floor level. Admin-only.
    fn command_spawn_ore(&mut self, client_id: ClientId, args: &[&str]) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        let mut chosen_ore: Option<&'static str> = None;
        let mut radius = DEFAULT_SPAWN_ORE_RADIUS;
        for arg in args {
            if let Some(ore) = parse_ore_token(arg) {
                chosen_ore = Some(ore);
            } else if let Ok(value) = arg.parse::<f32>() {
                radius = value;
            } else {
                return reply_warning(
                    client_id,
                    format!("unknown argument '{arg}'; expected ore type or radius"),
                );
            }
        }

        if !radius.is_finite() || radius <= 0.0 {
            return reply_warning(client_id, "radius must be a positive number");
        }
        let radius = radius.min(MAX_SPAWN_ORE_RADIUS);

        let player_position = client.controller.position;
        let mut rng = SmallRng::seed_from_time_and(client_id);
        let ore_id = chosen_ore.unwrap_or_else(|| random_ore_id(&mut rng));
        let position = random_position_around(player_position, radius, &mut rng);

        let node_id = self.allocate_resource_node_id();
        let spawn = WorldResourceNodeSpawn::new(
            node_id,
            ore_id,
            position,
            rng.next_f32() * std::f32::consts::TAU,
        );
        let Some(node) = spawn_resource_node(&spawn) else {
            return reply_warning(client_id, "could not build node: unknown ore type");
        };

        let distance = ((position.x - player_position.x).powi(2)
            + (position.z - player_position.z).powi(2))
        .sqrt();
        let label = resource_node_definition(ore_id)
            .map(|definition| definition.name)
            .unwrap_or("Ore");

        // Register with the chunk anchor index so the snapshot AoI
        // includes the spawn — without this, admin-spawned nodes are
        // invisible because per-chunk membership is the AoI source of
        // truth.
        let kind = ore_node_kind(ore_id);
        self.chunk_manager
            .track_resource_node(node_id, kind, position);
        self.resource_nodes.insert(node_id, node);

        reply_success(
            client_id,
            format!("spawned {label} {distance:.1}m away (id {node_id})"),
        )
    }

    /// `/test-kit` — debug shortcut that fills the player's bag with the
    /// full early-game kit:
    ///
    /// - Equipables (tools + deployables) → first empty actionbar slot,
    ///   falling back to inventory if the actionbar is already packed.
    /// - Resources (100 of each material) → first empty inventory slot
    ///   so they don't shove existing actionbar contents around.
    ///
    /// Admin only. Any items that can't fit (e.g. inventory full from
    /// earlier kits) are reported in the success toast — no silent loss.
    fn command_test_kit(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        // (item_id, quantity) tuples. Tools + deployables are equipables
        // and go to the actionbar first; resources go straight to the
        // inventory grid.
        const EQUIPABLES: &[&str] = &[
            BASIC_HATCHET_ID,
            BASIC_PICKAXE_ID,
            WORKBENCH_T1_ID,
            CRUDE_FURNACE_ID,
        ];
        const RESOURCES: &[&str] = &[
            WOOD_ID,
            STONE_ID,
            COAL_ID,
            IRON_ORE_ID,
            SULFUR_ORE_ID,
            FIBER_ID,
            PLANT_TWINE_ID,
            IRON_BAR_ID,
        ];
        const RESOURCE_QUANTITY: u16 = 100;

        let mut placed = 0u32;
        let mut overflow = 0u32;

        // Equipables: actionbar first → inventory fallback. Each one
        // is a stack of 1 (tools and deployables are equipable), so
        // we never need to merge them with an existing matching stack.
        for item_id in EQUIPABLES {
            let stack = ItemStack::new(*item_id, 1);
            if let Some(slot) = client
                .inventory
                .actionbar_slots
                .iter()
                .position(Option::is_none)
            {
                client.inventory.actionbar_slots[slot] = Some(stack);
                placed += 1;
            } else if add_stack_to_inventory(&mut client.inventory, stack).is_some() {
                overflow += 1;
            } else {
                placed += 1;
            }
        }

        // Resources: inventory only. Stack of 100 fits inside every
        // resource's stack limit (twine/wood/stone/etc cap at 200,
        // iron_bar caps at 100). We pick the first empty inventory
        // slot directly so granting a kit doesn't merge into the
        // player's existing piles in unpredictable order.
        for item_id in RESOURCES {
            let stack = ItemStack::new(*item_id, RESOURCE_QUANTITY);
            if let Some(slot) = client
                .inventory
                .inventory_slots
                .iter()
                .position(Option::is_none)
            {
                client.inventory.inventory_slots[slot] = Some(stack);
                placed += 1;
            } else {
                overflow += 1;
            }
        }

        let message = if overflow == 0 {
            format!("test kit granted ({placed} items)")
        } else {
            format!(
                "test kit granted ({placed} items, {overflow} couldn't fit; clear some inventory)"
            )
        };
        reply_success(client_id, message)
    }

    /// `/tp` — teleport every other connected player to the issuer's
    /// position. Bread-and-butter PvP-test command: drop both clients
    /// into the same arms-length so death/respawn/melee can be
    /// exercised without manually walking them together.
    ///
    /// Implementation: validate admin, snapshot the issuer's position,
    /// and for each other connected client move the server-side
    /// controller onto the issuer's spot, re-anchor the chunk
    /// membership, then push a `ServerMessage::Correction` so the
    /// client predictor snaps cleanly. The runtime applies the snap on
    /// a position delta above its 1 m threshold (see
    /// `apply_non_movement_correction`).
    fn command_teleport_all(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(issuer) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !issuer.is_admin {
            return reply_warning(client_id, "admin only");
        }
        let target_position = issuer.controller.position;
        let target_yaw = issuer.controller.yaw;

        let other_ids: Vec<ClientId> = self
            .clients
            .keys()
            .copied()
            .filter(|id| *id != client_id)
            .collect();

        let mut envelopes = Vec::new();
        let mut moved = 0u32;
        for other_id in &other_ids {
            let Some(other) = self.clients.get_mut(other_id) else {
                continue;
            };
            // Stamp the controller. Velocity is zeroed so the target
            // doesn't keep their inbound momentum and immediately slide
            // off the issuer's tile.
            other.controller.position = target_position;
            other.controller.velocity = Vec3Net::ZERO;
            // Keep the target's look direction so the camera doesn't
            // snap mid-frame; only the world position should change.
            // Re-anchor chunk membership so AoI replication sees the
            // new home immediately.
            self.chunk_manager
                .update_player_chunk(*other_id, target_position);

            // Synthesize a Correction so the client prediction follows.
            let state = crate::protocol::PlayerState {
                client_id: *other_id,
                position: target_position,
                velocity: Vec3Net::ZERO,
                yaw: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.yaw)
                    .unwrap_or(target_yaw),
                pitch: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.pitch)
                    .unwrap_or(0.0),
                health: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.health)
                    .unwrap_or(crate::protocol::MAX_HEALTH),
                grounded: true,
                last_processed_input: self
                    .clients
                    .get(other_id)
                    .map(|c| c.controller.last_processed_input)
                    .unwrap_or(0),
            };
            envelopes.push(ServerEnvelope {
                target: DeliveryTarget::Client(*other_id),
                message: ServerMessage::Correction(state),
            });
            moved += 1;
        }

        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Toast(ToastMessage::new(
                ToastKind::Success,
                if moved == 0 {
                    "no other players to teleport".to_owned()
                } else if moved == 1 {
                    "teleported 1 player to your position".to_owned()
                } else {
                    format!("teleported {moved} players to your position")
                },
            )),
        });
        envelopes
    }

    fn allocate_resource_node_id(&mut self) -> ResourceNodeId {
        let id = self.next_resource_node_id;
        self.next_resource_node_id = self.next_resource_node_id.saturating_add(1);
        id
    }
}

fn parse_ore_token(arg: &str) -> Option<&'static str> {
    match arg.to_ascii_lowercase().as_str() {
        "coal" => Some(COAL_NODE_ID),
        "iron" => Some(IRON_NODE_ID),
        "sulfur" | "sulphur" => Some(SULFUR_NODE_ID),
        _ => None,
    }
}

/// Map an ore `definition_id` to the matching `NodeKind` for chunk
/// membership bookkeeping. Defaults to `CoalOre` for unknown ids so the
/// node still ends up tracked rather than silently invisible — callers
/// only pass ids that came out of `parse_ore_token`, so the fallback
/// shouldn't fire in practice.
fn ore_node_kind(ore_id: &str) -> NodeKind {
    match ore_id {
        IRON_NODE_ID => NodeKind::IronOre,
        SULFUR_NODE_ID => NodeKind::SulfurOre,
        _ => NodeKind::CoalOre,
    }
}

fn random_ore_id(rng: &mut SmallRng) -> &'static str {
    const ORES: [&str; 3] = [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID];
    ORES[(rng.next_u32() as usize) % ORES.len()]
}

fn random_position_around(center: Vec3Net, radius: f32, rng: &mut SmallRng) -> Vec3Net {
    // Uniform sampling over an annulus (MIN_SPAWN_ORE_DISTANCE .. radius)
    // using inverse-CDF in r² so the points are area-uniform rather than
    // clustered toward the center.
    let inner = MIN_SPAWN_ORE_DISTANCE.min(radius * 0.5);
    let inner_sq = inner * inner;
    let outer_sq = radius * radius;
    let r = (inner_sq + rng.next_f32() * (outer_sq - inner_sq)).sqrt();
    let theta = rng.next_f32() * std::f32::consts::TAU;
    Vec3Net::new(
        center.x + r * theta.cos(),
        // Floor-aligned spawn — matches the hand-authored ore nodes in the
        // test world (all at y=0).
        0.0,
        center.z + r * theta.sin(),
    )
}

fn reply_success(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Success, text)),
    }]
}

fn reply_warning(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, text)),
    }]
}

/// Tiny xorshift32 PRNG. We don't need cryptographic randomness — just a
/// stream of "feels different" numbers between admin command invocations.
/// Avoids adding the `rand` crate just for one debug command.
pub(super) struct SmallRng {
    state: u32,
}

impl SmallRng {
    pub(super) fn seed_from_time_and(salt: u64) -> Self {
        // SystemTime fold-in for entropy across server restarts; the salt
        // mixes in client_id so two admins spawning in the same tick don't
        // get identical sequences.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.subsec_nanos())
            .unwrap_or(0);
        let mut state = (nanos ^ (salt as u32) ^ ((salt >> 32) as u32)).wrapping_mul(0x9E37_79B1);
        if state == 0 {
            state = 0xDEAD_BEEF;
        }
        Self { state }
    }

    pub(super) fn next_u32(&mut self) -> u32 {
        // Classic xorshift32 — short period for our purposes is fine.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    pub(super) fn next_f32(&mut self) -> f32 {
        // 24 bits of mantissa is plenty for picking world positions.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ore_token_accepts_canonical_and_alternate_spellings() {
        assert_eq!(parse_ore_token("coal"), Some(COAL_NODE_ID));
        assert_eq!(parse_ore_token("IRON"), Some(IRON_NODE_ID));
        assert_eq!(parse_ore_token("sulphur"), Some(SULFUR_NODE_ID));
        assert_eq!(parse_ore_token("granite"), None);
    }

    #[test]
    fn random_position_lands_inside_the_radius_and_outside_the_inner_ring() {
        let mut rng = SmallRng { state: 0x1234_5678 };
        let center = Vec3Net::new(10.0, 0.0, -3.0);
        for _ in 0..200 {
            let position = random_position_around(center, 12.0, &mut rng);
            let dx = position.x - center.x;
            let dz = position.z - center.z;
            let r = (dx * dx + dz * dz).sqrt();
            assert!(r <= 12.0 + 1e-3, "{r} should stay inside the outer ring");
            assert!(
                r >= MIN_SPAWN_ORE_DISTANCE.min(12.0 * 0.5) - 1e-3,
                "{r} should not land inside the inner cull"
            );
            assert_eq!(position.y, 0.0);
        }
    }

    #[test]
    fn small_rng_emits_changing_values() {
        let mut rng = SmallRng { state: 0xCAFE };
        let first = rng.next_u32();
        let second = rng.next_u32();
        assert_ne!(first, second);
    }

    #[test]
    fn test_kit_command_grants_full_kit_and_routes_equipables_to_actionbar() {
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };
        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok");

        // The singleplayer host gets admin status implicitly, so the
        // command should succeed on this freshly-connected client.
        let envelopes = server.apply_command(client_id, "/test-kit".to_owned());
        assert!(
            envelopes.iter().any(|envelope| matches!(
                &envelope.message,
                ServerMessage::Toast(toast) if matches!(toast.kind, ToastKind::Success)
            )),
            "test-kit should reply with a success toast"
        );

        let client = server
            .clients
            .get(&client_id)
            .expect("client still connected");

        // Tools + structures landed in the actionbar.
        let actionbar_ids: Vec<_> = client
            .inventory
            .actionbar_slots
            .iter()
            .filter_map(|slot| slot.as_ref().map(|s| s.item_id.as_ref().to_owned()))
            .collect();
        for required in [
            BASIC_HATCHET_ID,
            BASIC_PICKAXE_ID,
            WORKBENCH_T1_ID,
            CRUDE_FURNACE_ID,
        ] {
            assert!(
                actionbar_ids.iter().any(|id| id == required),
                "actionbar should contain {required}, got {actionbar_ids:?}",
            );
        }

        // Every resource type sits in the main inventory at the kit
        // quantity. Iron bar is capped at 100, others at 200, so 100
        // is always intact.
        for resource in [
            WOOD_ID,
            STONE_ID,
            COAL_ID,
            IRON_ORE_ID,
            SULFUR_ORE_ID,
            FIBER_ID,
            PLANT_TWINE_ID,
            IRON_BAR_ID,
        ] {
            let stack = client
                .inventory
                .inventory_slots
                .iter()
                .filter_map(|slot| slot.as_ref())
                .find(|stack| stack.item_id.as_ref() == resource)
                .unwrap_or_else(|| panic!("inventory should contain {resource}"));
            assert_eq!(stack.quantity, 100, "{resource} should be granted as 100");
        }
    }

    #[test]
    fn test_kit_command_refused_for_non_admin() {
        use crate::{
            protocol::{GAME_VERSION, PROTOCOL_VERSION},
            save::WorldSave,
            server::ServerSettings,
            steam::{AuthMode, offline_auth_token},
        };
        // Singleplayer host is admin; spin up a server with NO host so
        // the connecting client is a plain non-admin.
        let mut server = crate::server::GameServer::new(
            WorldSave::new("Test", None),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: None,
            },
        );
        let (client_id, _) = server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                7,
                "Tester".to_owned(),
                offline_auth_token(7),
            )
            .expect("connect ok");

        let envelopes = server.apply_command(client_id, "/test-kit".to_owned());
        assert!(
            envelopes.iter().any(|envelope| matches!(
                &envelope.message,
                ServerMessage::Toast(toast) if matches!(toast.kind, ToastKind::Warning)
            )),
            "non-admin should be rejected with a warning toast",
        );

        // Confirm no inventory mutation happened.
        let client = server.clients.get(&client_id).unwrap();
        let granted = client
            .inventory
            .inventory_slots
            .iter()
            .chain(client.inventory.actionbar_slots.iter())
            .any(|slot| slot.is_some());
        assert!(!granted, "non-admin must not have received any items");
    }
}
