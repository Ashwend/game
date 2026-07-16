//! Server authority for the base-building system: placing building blocks
//! via the building plan, and the hammer's repair / upgrade / demolish
//! actions. Snapping, costs, and the demolish window all resolve here; the
//! client only previews.
//!
//! Placement rules (shared geometry in [`crate::building`]):
//! - Foundations sit on the ground at the requested (player-facing) yaw. Near an
//!   existing foundation they snap onto its 3 m neighbour grid; the box
//!   overlap test rejects everything that isn't exactly on-grid.
//! - Wall-like pieces (wall, window wall, doorway) mount on a platform's
//!   edge sockets or stack on a wall below, one piece per socket.
//! - Ceilings snap to the cells flanking a wall's top edge and to cells
//!   adjacent to existing ceilings; stairs snap onto a platform cell.
//! - Building-vs-building collision resolves through socket occupancy,
//!   not boxes: walls legitimately touch their foundation and overlap each
//!   other at corners.
//! - Every snapped pose must additionally compute at least the minimum
//!   structural stability ([`super::stability`]); that is what stops
//!   pieces from being placed too far up or too far out on a ledge.

use crate::{
    building::{
        BuildingPiece, BuildingTier, building_item_id, building_max_health, cell_neighbor_sockets,
        placement_cost, platform_wall_sockets, positions_match, repair_cost, stairs_socket_on,
        upgrade_cost, wall_ceiling_sockets, wall_slot_blocked, wall_top_socket,
    },
    game_balance::{
        BUILDING_DEMOLISH_WINDOW_TICKS, BUILDING_MIN_PLACEMENT_STABILITY_PCT,
        BUILDING_REPAIR_COMBAT_LOCKOUT_TICKS, BUILDING_REPAIR_FRACTION_PCT,
    },
    inventory::{count_items_in_inventory, take_items_from_inventory},
    items::{DeployableKind, ToolKind, item_definition},
    protocol::{
        BuildingCommand, ClientId, DeployedEntityId, PlaceBuildingCommand, ToastKind, Vec3Net,
    },
};

use super::{GameServer, ServerEnvelope, deployables::DeployedEntity};

use crate::game_balance::{
    BUILDING_SNAP_TOLERANCE_M as SNAP_TOLERANCE_M,
    DEPLOYABLE_PLACEMENT_REACH_M as PLACEMENT_REACH_M,
};

impl GameServer {
    pub(super) fn apply_place_building_command(
        &mut self,
        client_id: ClientId,
        command: PlaceBuildingCommand,
    ) -> Vec<ServerEnvelope> {
        if !command.position.x.is_finite()
            || !command.position.y.is_finite()
            || !command.position.z.is_finite()
            || !command.yaw.is_finite()
        {
            return building_toast(client_id, ToastKind::Error, "Invalid placement".to_owned());
        }
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let feet = client.controller.position;
        let owner = client.account_id;
        if !feet.within_horizontal_range(command.position, PLACEMENT_REACH_M) {
            return building_toast(client_id, ToastKind::Warning, "Too far away".to_owned());
        }

        let piece = command.piece;
        // Re-derive the snapped pose server-side; the client preview is a
        // best guess.
        let pose = match piece {
            BuildingPiece::Wall | BuildingPiece::WindowWall | BuildingPiece::Doorway => {
                self.snap_wall_socket(piece, command.position, command.yaw)
            }
            BuildingPiece::Foundation => self.snap_foundation(command.position, command.yaw),
            BuildingPiece::Ceiling => self.snap_ceiling(command.position),
            BuildingPiece::Stairs => self.snap_stairs(command.position, command.yaw),
        };
        let (position, yaw) = match pose {
            Ok(pose) => pose,
            Err(reason) => return building_toast(client_id, ToastKind::Warning, reason.to_owned()),
        };

        // Ruins are shared loot spots: no player construction inside a
        // footprint plus the placement margin (nobody walls in the salvage
        // chests). Tested at the snapped centre; the margin comfortably
        // covers a piece's half-extent.
        if self.ruin_blocks_placement(position) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Too close to a ruin to build".to_owned(),
            );
        }

        // Building privilege: a Tool Cupboard projects a claim over its
        // base + a margin ring. Non-authorized players can't build anything
        // inside it, every piece including the first (twig) tier is
        // refused, so a claimed base fully locks outsiders out. The test is
        // footprint-aware: the whole piece (slab, wall span, stair flight)
        // must clear the claim, not just its snap centre, so a wall can't be
        // butted up against the boundary to poke into protected ground.
        let footprint = crate::building::building_collider_blocks(piece, position, yaw);
        if self.claim_blocks_footprint(&footprint, owner) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "This area is claimed by someone else".to_owned(),
            );
        }

        // Box-overlap test against non-building deployables (don't bisect
        // someone's furnace) and against other foundations when placing a
        // foundation. Wall-like-vs-building overlap is resolved by socket
        // occupancy above; see `any_deployable_overlaps` for why.
        // `new` seeds stability at 100 as a placeholder; the post-insert
        // refresh computes the real value (and the gate below already proved
        // it's above the minimum).
        let candidate = DeployedEntity::new(
            crate::items::intern_item_id(building_item_id(piece)),
            DeployableKind::Building {
                piece,
                tier: BuildingTier::Sticks,
            },
            position,
            yaw,
            building_max_health(piece, BuildingTier::Sticks),
            Some(owner),
            self.tick,
        );
        let blocks = candidate.resolved_collider_blocks();
        let obstruction = self.deployed_entities.values().any(|existing| {
            let skip = match existing.kind {
                // Walls touch their platform and corner-overlap their
                // neighbours by construction; occupancy already rejected
                // a same-socket duplicate. Stairs and ceilings likewise
                // legitimately meet the wall/door plane at their cell
                // edges (a stacked wall shares the ceiling slab's height
                // band), but must still collide with platforms, other
                // stairs, and each other (a roof over a stairs cell
                // blocks the flight; duplicate ceilings overlap).
                DeployableKind::Building {
                    piece: existing_piece,
                    ..
                } => {
                    piece.is_wall_like()
                        || (matches!(piece, BuildingPiece::Stairs | BuildingPiece::Ceiling)
                            && existing_piece.is_wall_like())
                }
                DeployableKind::Door { .. } => {
                    piece.is_wall_like()
                        || matches!(piece, BuildingPiece::Stairs | BuildingPiece::Ceiling)
                }
                _ => false,
            };
            !skip
                && existing
                    .resolved_collider_blocks()
                    .iter()
                    .any(|block| blocks.iter().any(|candidate| candidate.overlaps(*block)))
        });
        if obstruction {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Something is in the way".to_owned(),
            );
        }

        // Structural stability: the snapped pose must compute enough
        // support, this is what caps tower height and ledge overhang.
        let stability = self.building_candidate_stability(piece, position, yaw);
        if stability < BUILDING_MIN_PLACEMENT_STABILITY_PCT {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Not enough structural support here".to_owned(),
            );
        }

        // Materials: verify the full cost first, then take it, partial
        // drains would eat resources on a failed placement.
        let (cost_item, cost_quantity) = placement_cost(piece);
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if count_items_in_inventory(&client.inventory, cost_item) < u32::from(cost_quantity) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                format!("Needs {cost_quantity} {}", material_name(cost_item)),
            );
        }
        take_items_from_inventory(&mut client.inventory, cost_item, cost_quantity);

        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id.0 = self.next_deployed_entity_id.0.saturating_add(1);
        let entity = DeployedEntity { id, ..candidate };
        self.insert_deployed_entity(id, entity);
        self.chunk_manager.track_deployed_entity(id, position);
        // A new piece changes the support graph (it may carry neighbours
        // placed before it); recompute.
        self.refresh_structural_stability();

        // Report the spend, not the piece: the placed structure is right there
        // in front of the player, but the resource subtraction is the feedback
        // they can't otherwise see.
        building_toast(
            client_id,
            ToastKind::Success,
            format!("-{cost_quantity} {}", material_name(cost_item)),
        )
    }

    /// Snap a foundation request: free placement at the requested (player-facing)
    /// yaw anywhere inside the raise band (ground level up to
    /// `FOUNDATION_RAISE_MAX_M`, aim-driven on the client), or, when the
    /// player aims near an existing foundation, onto that foundation's
    /// 3 m neighbour grid (inheriting its yaw *and* height so bases stay
    /// square and level).
    fn snap_foundation(
        &self,
        requested: Vec3Net,
        requested_yaw: f32,
    ) -> Result<(Vec3Net, f32), &'static str> {
        if requested.y < -crate::game_balance::FOUNDATION_SINK_MAX_M
            || requested.y > crate::game_balance::FOUNDATION_RAISE_MAX_M + 0.05
        {
            return Err("Foundations must sit near the ground");
        }
        let mut best: Option<(f32, Vec3Net, f32)> = None;
        for existing in self.deployed_entities.values() {
            let DeployableKind::Building {
                piece: BuildingPiece::Foundation,
                ..
            } = existing.kind
            else {
                continue;
            };
            for socket in cell_neighbor_sockets(existing.position, existing.yaw) {
                let dx = socket.position.x - requested.x;
                let dz = socket.position.z - requested.z;
                let distance = (dx * dx + dz * dz).sqrt();
                if distance <= SNAP_TOLERANCE_M
                    && best
                        .as_ref()
                        .is_none_or(|(current, _, _)| distance < *current)
                {
                    best = Some((distance, socket.position, socket.yaw));
                }
            }
        }
        if let Some((_, position, yaw)) = best {
            // A foundation may already occupy the snapped cell; the box
            // overlap test downstream would catch it, but a same-position
            // duplicate "touches" rather than overlaps, so check here.
            let occupied = self.deployed_entities.values().any(|existing| {
                matches!(
                    existing.kind,
                    DeployableKind::Building {
                        piece: BuildingPiece::Foundation,
                        ..
                    }
                ) && positions_match(existing.position, position)
            });
            if occupied {
                return Err("That spot already has a foundation");
            }
            return Ok((position, yaw));
        }
        Ok((
            // Free placement keeps the aim-driven height (validated to
            // the raise band above); negative wobble clamps to ground so
            // a slightly sunk request doesn't bury the slab.
            Vec3Net::new(requested.x, requested.y.max(0.0), requested.z),
            // Snap the yaw to the quarter-turn grid. Building yaw is always
            // cardinal (see the `building` module docs): edge wall sockets,
            // colliders, and the block grid all assume it. A free foundation
            // left at its arbitrary player-facing yaw renders rotated while its
            // walls (whose sockets are quarter-snapped) align to the world axes,
            // so every wall sits skewed on the slab. Snapping here keeps the slab
            // and the walls it carries square.
            crate::building::snap_yaw_quarter_turn(requested_yaw),
        ))
    }

    /// Snap a wall-like request onto the nearest free wall socket: a
    /// platform's edge (foundation or ceiling) or the top of an existing
    /// wall-like piece (stacking upward). Rejects when no socket is in
    /// tolerance or the socket already hosts a wall-like piece. Distance
    /// is 3D: stacked storeys put sockets at identical XZ, so height has
    /// to disambiguate.
    fn snap_wall_socket(
        &self,
        piece: BuildingPiece,
        requested: Vec3Net,
        requested_yaw: f32,
    ) -> Result<(Vec3Net, f32), &'static str> {
        debug_assert!(piece.is_wall_like());
        let mut best: Option<(f32, Vec3Net, f32)> = None;
        let consider = |position: Vec3Net, yaw: f32, best: &mut Option<(f32, Vec3Net, f32)>| {
            let distance = distance_3d(position, requested);
            if distance <= SNAP_TOLERANCE_M
                && best
                    .as_ref()
                    .is_none_or(|(current, _, _)| distance < *current)
            {
                *best = Some((distance, position, yaw));
            }
        };
        for existing in self.deployed_entities.values() {
            let DeployableKind::Building {
                piece: existing_piece,
                ..
            } = existing.kind
            else {
                continue;
            };
            if let Some(sockets) =
                platform_wall_sockets(existing_piece, existing.position, existing.yaw)
            {
                for socket in sockets {
                    consider(socket.position, socket.yaw, &mut best);
                }
            }
            if let Some(top) = wall_top_socket(existing_piece, existing.position, existing.yaw) {
                consider(top.position, top.yaw, &mut best);
            }
        }
        let Some((_, position, yaw)) = best else {
            return Err("Walls mount on a platform edge or atop a wall");
        };
        let _ = requested_yaw; // The socket owns the orientation.
        let occupied = self.deployed_entities.values().any(|existing| {
            matches!(existing.kind, DeployableKind::Building { piece, .. } if piece.is_wall_like())
                && wall_slot_blocked(existing.position, existing.yaw, position, yaw)
        });
        if occupied {
            return Err("That edge already has a wall");
        }
        Ok((position, yaw))
    }

    /// Snap a ceiling request onto the nearest carried cell: the cells
    /// flanking a wall's top edge, or a cell adjacent to an existing
    /// ceiling (extending a ledge). Whether the spot has enough support
    /// is the stability gate's call, not the snapper's.
    fn snap_ceiling(&self, requested: Vec3Net) -> Result<(Vec3Net, f32), &'static str> {
        let mut best: Option<(f32, Vec3Net, f32)> = None;
        let consider = |position: Vec3Net, yaw: f32, best: &mut Option<(f32, Vec3Net, f32)>| {
            let distance = distance_3d(position, requested);
            if distance <= SNAP_TOLERANCE_M
                && best
                    .as_ref()
                    .is_none_or(|(current, _, _)| distance < *current)
            {
                *best = Some((distance, position, yaw));
            }
        };
        for existing in self.deployed_entities.values() {
            let DeployableKind::Building {
                piece: existing_piece,
                ..
            } = existing.kind
            else {
                continue;
            };
            if let Some(cells) =
                wall_ceiling_sockets(existing_piece, existing.position, existing.yaw)
            {
                for cell in cells {
                    consider(cell.position, cell.yaw, &mut best);
                }
            }
            if matches!(existing_piece, BuildingPiece::Ceiling) {
                for socket in cell_neighbor_sockets(existing.position, existing.yaw) {
                    consider(socket.position, socket.yaw, &mut best);
                }
            }
        }
        let Some((_, position, yaw)) = best else {
            return Err("Ceilings rest on walls or extend another ceiling");
        };
        // Duplicate ceilings share identical boxes, which the overlap
        // test downstream rejects; no separate occupancy check needed.
        Ok((position, yaw))
    }

    /// Snap a stairs request onto the top surface of the nearest
    /// platform cell. The flight's rise direction is the player's
    /// quarter-snapped yaw; R rotates the ghost.
    fn snap_stairs(
        &self,
        requested: Vec3Net,
        requested_yaw: f32,
    ) -> Result<(Vec3Net, f32), &'static str> {
        let mut best: Option<(f32, Vec3Net, f32)> = None;
        for existing in self.deployed_entities.values() {
            let DeployableKind::Building {
                piece: existing_piece,
                ..
            } = existing.kind
            else {
                continue;
            };
            let Some(socket) = stairs_socket_on(existing_piece, existing.position, requested_yaw)
            else {
                continue;
            };
            let distance = distance_3d(socket.position, requested);
            if distance <= SNAP_TOLERANCE_M
                && best
                    .as_ref()
                    .is_none_or(|(current, _, _)| distance < *current)
            {
                best = Some((distance, socket.position, socket.yaw));
            }
        }
        let Some((_, position, yaw)) = best else {
            return Err("Stairs stand on a foundation or ceiling");
        };
        Ok((position, yaw))
    }

    pub(super) fn apply_building_command(
        &mut self,
        client_id: ClientId,
        command: BuildingCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            BuildingCommand::Repair { id } => self.repair_building(client_id, id),
            BuildingCommand::Upgrade { id } => self.upgrade_building(client_id, id),
            BuildingCommand::Demolish { id } => self.demolish_building(client_id, id),
        }
    }

    /// One hammer repair hit: requires the hammer in hand and the target
    /// in melee range; consumes tier materials from the swinger and
    /// restores a fraction of max HP. Anyone may repair (helping a
    /// neighbour patch their wall is fine), the cost lands on the swinger.
    fn repair_building(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(hammer) = self.hammer_in_range(client_id, id, true) else {
            return Vec::new();
        };
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        // Combat lockout: damage must stick for its window before the piece
        // can be patched back up (see BUILDING_REPAIR_COMBAT_LOCKOUT_TICKS).
        if let Some(remaining) = self.repair_lockout_remaining_ticks(id) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                format!(
                    "Recently damaged, repairable in {}",
                    format_ticks_mmss(remaining)
                ),
            );
        }
        let (cost_item, cost_quantity) = match entity.kind {
            DeployableKind::Building { tier, .. } => repair_cost(tier),
            // Doors repair in their own material: hewn logs for the wood
            // door and the shutter, iron bars for the iron one.
            DeployableKind::Door { variant } => match variant {
                crate::items::DoorVariant::HewnLog | crate::items::DoorVariant::Shutter => {
                    (crate::items::HEWN_LOG_ID, 1)
                }
                crate::items::DoorVariant::Iron => (crate::items::IRON_BAR_ID, 1),
            },
            // Crafted deployables (furnace, workbench, bag, boxes)
            // repair with their recipe's primary material; see
            // `repair_material_for`.
            _ => match crate::crafting::repair_material_for(&entity.item_id) {
                Some(cost) => cost,
                None => return Vec::new(),
            },
        };
        if entity.health >= entity.max_health {
            return building_toast(
                client_id,
                ToastKind::Info,
                "Already at full health".to_owned(),
            );
        }
        let restore = (entity
            .max_health
            .saturating_mul(BUILDING_REPAIR_FRACTION_PCT)
            / 100)
            .max(1);

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if count_items_in_inventory(&client.inventory, cost_item) < u32::from(cost_quantity) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                format!("Repair needs {cost_quantity} {}", material_name(cost_item)),
            );
        }
        take_items_from_inventory(&mut client.inventory, cost_item, cost_quantity);
        client.next_gather_tick = self.tick + hammer.cooldown_ticks.max(1);

        if let Some(entity) = self.deployed_entity_mut(id) {
            entity.health = entity.health.saturating_add(restore).min(entity.max_health);
        }
        // The repair connected, the hammer wears like any working tool.
        self.consume_active_tool_durability(client_id)
    }

    /// Upgrade a building block to the next tier: authorized-only (builder
    /// or anyone on the covering Tool Cupboard), costs the
    /// target tier's materials, refills HP, and restarts the demolish
    /// window (fresh construction, fresh grace period).
    fn upgrade_building(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        if self.hammer_in_range(client_id, id, false).is_none() {
            return Vec::new();
        }
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let account = client.account_id;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        if !self.building_modify_allowed(entity.position, account, entity.owner == Some(account)) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Only authorized players can upgrade this".to_owned(),
            );
        }
        // Upgrades share the repair combat lockout: an upgrade refills HP to
        // the new tier's full, a bigger heal than any repair hit, so a
        // recently-hit piece can't be upgraded out from under a raid either.
        if let Some(remaining) = self.repair_lockout_remaining_ticks(id) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                format!(
                    "Recently damaged, upgradeable in {}",
                    format_ticks_mmss(remaining)
                ),
            );
        }
        let DeployableKind::Building { piece, tier } = entity.kind else {
            return Vec::new();
        };
        let Some(next_tier) = tier.next() else {
            return building_toast(client_id, ToastKind::Info, "Already top tier".to_owned());
        };
        let (cost_item, cost_quantity) = upgrade_cost(piece, next_tier);

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if count_items_in_inventory(&client.inventory, cost_item) < u32::from(cost_quantity) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                format!("Upgrade needs {cost_quantity} {}", material_name(cost_item)),
            );
        }
        take_items_from_inventory(&mut client.inventory, cost_item, cost_quantity);

        let tick = self.tick;
        if let Some(entity) = self.deployed_entity_mut(id) {
            let max = building_max_health(piece, next_tier);
            entity.kind = DeployableKind::Building {
                piece,
                tier: next_tier,
            };
            entity.max_health = max;
            entity.health = max;
            entity.placed_at_tick = tick;
        }
        building_toast(
            client_id,
            ToastKind::Success,
            format!("Upgraded to {}", next_tier.label()),
        )
    }

    /// Demolish a building block or door: only an authorized player (the
    /// builder, or anyone on the covering Tool Cupboard), and only while
    /// the demolish window since placement/upgrade is open.
    fn demolish_building(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        if self.hammer_in_range(client_id, id, false).is_none() {
            return Vec::new();
        }
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let account = client.account_id;
        let Some(entity) = self.deployed_entities.get(&id) else {
            return Vec::new();
        };
        if !matches!(
            entity.kind,
            DeployableKind::Building { .. } | DeployableKind::Door { .. }
        ) {
            return Vec::new();
        }
        if !self.building_modify_allowed(entity.position, account, entity.owner == Some(account)) {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Only authorized players can demolish this".to_owned(),
            );
        }
        if self.tick.saturating_sub(entity.placed_at_tick) > BUILDING_DEMOLISH_WINDOW_TICKS {
            return building_toast(
                client_id,
                ToastKind::Warning,
                "Too late to demolish, it has set".to_owned(),
            );
        }
        let label = entity.kind.label();
        self.destroy_deployed_entity(id);
        building_toast(client_id, ToastKind::Success, format!("Demolished {label}"))
    }

    /// Common gate for hammer actions: the sender must exist, hold the
    /// hammer, and stand in melee range of the target. `respect_cooldown`
    /// applies the swing cadence (repairs ride the swing loop; the wheel
    /// actions are deliberate single clicks). Returns the hammer's tool
    /// profile on success.
    fn hammer_in_range(
        &self,
        client_id: ClientId,
        id: DeployedEntityId,
        respect_cooldown: bool,
    ) -> Option<crate::items::ToolProfile> {
        let client = self.clients.get(&client_id)?;
        if respect_cooldown && self.tick < client.next_gather_tick {
            return None;
        }
        let tool = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.tool)?;
        if tool.kind != ToolKind::Hammer {
            return None;
        }
        let entity = self.deployed_entities.get(&id)?;
        // Hammer work uses the placement reach, not the melee range:
        // building pieces are 3 m spans whose `position` sits on the
        // far edge of the foundation you're standing on, so the melee
        // radius would reject taps on a wall right in front of you.
        client
            .controller
            .position
            .within_horizontal_range(entity.position, PLACEMENT_REACH_M)
            .then_some(tool)
    }

    /// Ticks left on the combat repair/upgrade lockout for `id`, `None` when
    /// the piece may be repaired or upgraded. Reads the transient
    /// `recently_damaged` stamp the three damage paths (tool swing,
    /// projectile, blast) write.
    fn repair_lockout_remaining_ticks(&self, id: DeployedEntityId) -> Option<u64> {
        let last = *self.recently_damaged.get(&id)?;
        let elapsed = self.tick.saturating_sub(last);
        let remaining = BUILDING_REPAIR_COMBAT_LOCKOUT_TICKS.saturating_sub(elapsed);
        (remaining > 0).then_some(remaining)
    }
}

fn distance_3d(a: Vec3Net, b: Vec3Net) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn material_name(item_id: &str) -> &'static str {
    item_definition(item_id)
        .map(|definition| definition.name)
        .unwrap_or("materials")
}

/// Render a tick count as "m:ss" for the lockout toasts, rounding the tail
/// second up so a lockout never reads "0:00" while still active.
fn format_ticks_mmss(ticks: u64) -> String {
    let secs = (ticks as f32 / crate::protocol::SERVER_TICK_RATE_HZ).ceil() as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

fn building_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    super::toasts::toast(client_id, kind, text)
}
