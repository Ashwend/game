//! Server-authoritative state for placed structures (workbenches,
//! furnaces, future deployables).
//!
//! Storage shape mirrors `resource_nodes`: a `HashMap<DeployedEntityId, _>`
//! owned by `GameServer`, with chunk membership tracked separately in the
//! chunk manager so AoI snapshots filter by visible chunk.
//!
//! Placement validation lives here so the server is the single source of
//! truth for "can this go here?", the client only shows a best-guess
//! preview. The same `placement_validation` helpers run on save load to
//! drop entries that no longer fit (e.g. a deployable saved before the
//! world geometry shifted).

use std::collections::HashMap;

use crate::{
    crafting::RecipeStation,
    items::{
        DeployableKind, DeployableProfile, HANDS_TOOL, ItemId, ToolKind, item_definition,
        tool_effectiveness_pct,
    },
    protocol::{
        ClientId, DamageDeployableCommand, DeployedEntityId, PlaceDeployableCommand, ServerMessage,
        ToastKind, ToastMessage, Vec3Net,
    },
    world::WorldBlock,
};

use super::{DeliveryTarget, GameServer, ServerEnvelope, inventory::take_items_from_inventory};

use crate::game_balance::{
    DEPLOYABLE_DAMAGE_PER_GATHER_POINT as DAMAGE_PER_GATHER_POINT,
    DEPLOYABLE_DAMAGE_RANGE_M as DAMAGE_RANGE_M, DEPLOYABLE_PLACEMENT_REACH_M as PLACEMENT_REACH_M,
};

/// Authoritative record of a placed structure. The id is server-assigned
/// and stable for the entity's lifetime.
#[derive(Debug, Clone)]
pub(crate) struct DeployedEntity {
    pub(super) id: DeployedEntityId,
    pub(super) item_id: ItemId,
    pub(super) kind: DeployableKind,
    pub(super) position: Vec3Net,
    pub(super) yaw: f32,
    pub(super) health: u32,
    pub(super) max_health: u32,
    /// account id of the player who placed this entity, or `None` for
    /// world-spawned structures. Gates damage on non-raidable kinds and
    /// gates the hammer's upgrade/demolish plus bag rename/pickup.
    pub(super) owner: Option<crate::protocol::AccountId>,
    /// Furnace-only state. `None` for non-furnaces; the place handler
    /// initialises a default `FurnaceState` for placed furnaces.
    pub(super) furnace: Option<super::furnace::FurnaceState>,
    /// Server tick this entity was placed (refreshed by tier upgrades).
    /// Drives the hammer's demolish window for building blocks.
    pub(super) placed_at_tick: u64,
    /// Door-only code-lock state. `None` for every other kind.
    pub(super) door: Option<super::door::DoorState>,
    /// Player-given display name (sleeping bags today). Replicated via
    /// `DeployableLabel` and shown on the respawn screen.
    pub(super) label: Option<String>,
    /// Storage-box-only contents. `None` for every other kind; the
    /// place handler initialises an empty grid for placed boxes.
    pub(super) storage: Option<super::storage_box::StorageBoxState>,
    /// Torch-only burn state (lit flag + countdown). `None` for every
    /// other kind; the place handler initialises a full burn for torches.
    pub(super) torch: Option<super::torch::TorchState>,
    /// Structural stability percentage (0-100). Building pieces and
    /// doors get theirs from the support graph (see
    /// [`super::stability`]); free-standing deployables sit on the
    /// ground and stay at 100. Not persisted: recomputed on load.
    pub(super) stability: u8,
}

impl DeployedEntity {
    /// Solid AABBs for placement-overlap, the spawn-safety grid, and the
    /// client's movement grid. Single square-footprint box for classic
    /// deployables; the building module's box layouts (with real openings
    /// and yaw-aware extents) for building blocks and doors, the same
    /// boxes the client builds, so the two stay aligned. A door's box
    /// follows its hinge state (closed plane vs swung-open panel).
    pub(super) fn collider_blocks(&self, profile: DeployableProfile) -> Vec<WorldBlock> {
        match self.kind {
            DeployableKind::Building { piece, .. } => {
                crate::building::building_collider_blocks(piece, self.position, self.yaw)
            }
            DeployableKind::Door => {
                let open = self.door.as_ref().is_some_and(|door| door.open);
                crate::building::door_collider_blocks(self.position, self.yaw, open)
            }
            _ => {
                let center = Vec3Net::new(
                    self.position.x,
                    self.position.y + profile.collider_half_height,
                    self.position.z,
                );
                let half = Vec3Net::new(
                    profile.collider_half_width,
                    profile.collider_half_height,
                    profile.collider_half_width,
                );
                vec![WorldBlock::new(center, half)]
            }
        }
    }

    /// [`Self::collider_blocks`] with the item-id profile lookup folded in.
    /// Empty when the item id no longer resolves a deployable profile (skip
    /// it rather than crash). Used by the spawn-safety grid.
    pub(super) fn resolved_collider_blocks(&self) -> Vec<WorldBlock> {
        let Some(profile) = item_definition(&self.item_id).and_then(|def| def.deployable) else {
            return Vec::new();
        };
        self.collider_blocks(profile)
    }
}

impl GameServer {
    pub(super) fn apply_place_deployable_command(
        &mut self,
        client_id: ClientId,
        command: PlaceDeployableCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(definition) = item_definition(&command.item_id) else {
            return place_toast(client_id, ToastKind::Error, "Unknown item".to_owned());
        };
        let Some(profile) = definition.deployable else {
            return place_toast(
                client_id,
                ToastKind::Warning,
                format!("{} can't be placed", definition.name),
            );
        };
        // Doors mount in doorway openings via `DoorCommand::Place` (which
        // carries the lock code); building blocks go through the building
        // plan. Neither rides the free-placement path.
        if matches!(
            profile.kind,
            DeployableKind::Door | DeployableKind::Building { .. }
        ) {
            return place_toast(
                client_id,
                ToastKind::Warning,
                "That can't be placed freely".to_owned(),
            );
        }
        // Torches take their own path: they can mount on the side of a wall
        // (no floor-surface requirement) and carry a burn timer.
        if matches!(profile.kind, DeployableKind::Torch { .. }) {
            return self.place_torch(client_id, command, definition, profile);
        }

        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };

        // Reach check: feet-to-target distance must not exceed
        // PLACEMENT_REACH_M, and the target must stand on a real surface
        // (the world floor or a building platform's walkable top) so the
        // player can't snipe a structure into thin air.
        let feet = client.controller.position;
        if !feet.within_horizontal_range(command.position, PLACEMENT_REACH_M) {
            return place_toast(client_id, ToastKind::Warning, "Too far away".to_owned());
        }
        if !self.valid_deployable_surface(command.position) {
            return place_toast(
                client_id,
                ToastKind::Warning,
                "Place on the ground or on a floor".to_owned(),
            );
        }
        if !command.position.x.is_finite()
            || !command.position.y.is_finite()
            || !command.position.z.is_finite()
            || !command.yaw.is_finite()
        {
            return place_toast(client_id, ToastKind::Error, "Invalid placement".to_owned());
        }

        // Overlap check: a candidate AABB at the requested pose mustn't
        // intersect any other placed structure's AABB. Drop overlap test
        // is left to gather (drops sit lower than typical deployables);
        // resource nodes already enforce their own collision so the
        // player can't hammer a workbench inside a tree.
        let owner_account_id = client.account_id;
        let candidate = DeployedEntity {
            id: 0,
            item_id: command.item_id.clone(),
            kind: profile.kind,
            position: command.position,
            yaw: command.yaw,
            health: profile.max_health,
            max_health: profile.max_health,
            owner: Some(owner_account_id),
            furnace: None,
            placed_at_tick: self.tick,
            door: None,
            label: None,
            stability: 100,
            storage: None,
            torch: None,
        };
        let candidate_blocks = candidate.collider_blocks(profile);
        if self.any_deployable_overlaps(&candidate_blocks, None) {
            return place_toast(
                client_id,
                ToastKind::Warning,
                "Something is in the way".to_owned(),
            );
        }

        // Recipe-station-style gating *of placement itself* is intentionally
        // not enforced here, gating happens at crafting time. A player who
        // somehow has a furnace in inventory (admin spawn, future trade)
        // can still place it without owning a workbench.

        // Consume one item from the player's inventory. Re-borrow mutably
        // now that the immutable client reference is no longer live.
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let removed = take_items_from_inventory(&mut client.inventory, definition.id, 1);
        if removed != 1 {
            return place_toast(
                client_id,
                ToastKind::Warning,
                format!("You don't have a {}", definition.name),
            );
        }

        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id = self.next_deployed_entity_id.saturating_add(1);
        let mut entity = DeployedEntity { id, ..candidate };
        // Furnaces ship with an empty operational state so the client
        // can render the slot grid the moment the entity appears in
        // the snapshot. Storage boxes likewise get their empty grid.
        // Other deployables stay `None`.
        if matches!(entity.kind, DeployableKind::Furnace { .. }) {
            entity.furnace = Some(super::furnace::FurnaceState::default());
        }
        if let DeployableKind::StorageBox { tier } = entity.kind {
            entity.storage = Some(super::storage_box::StorageBoxState::new(tier));
        }
        let position = entity.position;
        self.insert_deployed_entity(id, entity);
        self.chunk_manager.track_deployed_entity(id, position);

        place_toast(
            client_id,
            ToastKind::Success,
            format!("Placed {}", definition.name),
        )
    }

    /// Place a torch. Unlike the other free deployables, a torch can mount on
    /// the side of a wall (the client's free-view raycast found it), so the
    /// floor-surface and overlap checks are relaxed: a floor torch must still
    /// stand on a real surface, a wall torch trusts the client raycast
    /// (placement is reach-gated, and a stray floating torch is only
    /// cosmetic). The mount is baked into the kind so it replicates for free
    /// via the immutable `Deployable` component.
    fn place_torch(
        &mut self,
        client_id: ClientId,
        command: PlaceDeployableCommand,
        definition: &crate::items::ItemDefinition,
        profile: DeployableProfile,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let feet = client.controller.position;
        if !feet.within_horizontal_range(command.position, PLACEMENT_REACH_M) {
            return place_toast(client_id, ToastKind::Warning, "Too far away".to_owned());
        }
        if !command.position.x.is_finite()
            || !command.position.y.is_finite()
            || !command.position.z.is_finite()
            || !command.yaw.is_finite()
        {
            return place_toast(client_id, ToastKind::Error, "Invalid placement".to_owned());
        }
        // Floor torches still need a real surface under them; wall torches
        // sit against a wall the client already found.
        if !command.wall_mounted && !self.valid_deployable_surface(command.position) {
            return place_toast(
                client_id,
                ToastKind::Warning,
                "Place on the ground, a floor, or a wall".to_owned(),
            );
        }

        let owner_account_id = client.account_id;
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let removed = take_items_from_inventory(&mut client.inventory, definition.id, 1);
        if removed != 1 {
            return place_toast(
                client_id,
                ToastKind::Warning,
                format!("You don't have a {}", definition.name),
            );
        }

        let id = self.next_deployed_entity_id;
        self.next_deployed_entity_id = self.next_deployed_entity_id.saturating_add(1);
        let entity = DeployedEntity {
            id,
            item_id: command.item_id.clone(),
            kind: DeployableKind::Torch {
                wall: command.wall_mounted,
            },
            position: command.position,
            yaw: command.yaw,
            health: profile.max_health,
            max_health: profile.max_health,
            owner: Some(owner_account_id),
            furnace: None,
            placed_at_tick: self.tick,
            door: None,
            label: None,
            stability: 100,
            storage: None,
            torch: Some(super::torch::TorchState::new()),
        };
        let position = entity.position;
        self.insert_deployed_entity(id, entity);
        self.chunk_manager.track_deployed_entity(id, position);

        place_toast(
            client_id,
            ToastKind::Success,
            format!("Placed {}", definition.name),
        )
    }

    pub(super) fn apply_damage_deployable_command(
        &mut self,
        client_id: ClientId,
        command: DamageDeployableCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        // Honour the same per-tool cooldown that gathering uses so a
        // damage swing can't fire faster than the tool's swing cadence.
        if self.tick < client.next_gather_tick {
            return Vec::new();
        }
        let player_pos = client.controller.position;
        let tool = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .and_then(|def| def.tool)
            .unwrap_or(HANDS_TOOL);
        // Bare hands don't damage placed structures, the client gates
        // this too, but defence in depth.
        if tool.kind == ToolKind::Hands {
            return Vec::new();
        }

        let attacker_account_id = client.account_id;
        let attacker_is_admin = client.is_admin;
        let Some(entity) = self.deployed_entities.get(&command.id) else {
            return Vec::new();
        };
        // Ownership gate: raid targets (building blocks, doors, sleeping
        // bags) are damageable by anyone, that's what makes raiding a
        // game. World-spawned entities (`owner = None`) likewise. Other
        // player-placed entities (workbench, furnace) can only be damaged
        // by their placer, except admins, who can demolish anyone's
        // structures for moderation.
        if !attacker_is_admin
            && !entity.kind.raidable()
            && let Some(owner) = entity.owner
            && owner != attacker_account_id
        {
            return Vec::new();
        }
        // Melee range is measured to the structure's *surface*, not its
        // centre: a foundation is a 3 m slab whose centre sits out of
        // range while its edge is right at the player's feet, and the
        // client's swing targeting (ray vs collider boxes) already hits
        // the surface. Centre-distance here silently dropped those hits.
        if !within_horizontal_range_of_blocks(
            player_pos,
            &entity.resolved_collider_blocks(),
            DAMAGE_RANGE_M,
        ) {
            return Vec::new();
        }
        // Tool-vs-material multiplier, hatchet eats wood, pickaxe
        // eats stone, mismatched proper tools still chip away but at
        // ~1/3 the rate of the matched pairing.
        let multiplier_pct = tool_effectiveness_pct(tool.kind, entity.kind.material());
        let base = (tool.gather_amount as u32).saturating_mul(DAMAGE_PER_GATHER_POINT);
        let damage = base.saturating_mul(multiplier_pct) / 100;

        // Mutable borrow for the actual decrement. We re-fetch instead
        // of holding the earlier `entity` reference across the cooldown
        // write below, borrow-checker convenience, not a hot path.
        // `deployed_entity_mut` flags the entity dirty so the mirror
        // re-syncs `DeployableHealth` next pass.
        let Some(entity) = self.deployed_entity_mut(command.id) else {
            return Vec::new();
        };
        entity.health = entity.health.saturating_sub(damage);
        let dead = entity.health == 0;

        // Apply the swing cooldown after a successful hit so spamming
        // damage swings doesn't bypass the gather throttle.
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.next_gather_tick = self.tick + tool.cooldown_ticks.max(1);
        }

        if dead {
            self.destroy_deployed_entity(command.id);
        }
        // Survivor health change replicates via the ECS mirror →
        // Lightyear's `DeployableHealth` diff. See
        // [Networking § Replication](../../docs/networking.md#replication).

        // The swing connected with the structure, so the tool wears.
        self.consume_active_tool_durability(client_id)
    }

    /// Remove a placed structure entirely (gameplay death + tracker
    /// untrack), then collapse everything it was holding up via the
    /// stability recompute (see [`super::stability`]). Players who had
    /// it open as a furnace get kicked back to the world view
    /// automatically because the snapshot's `open_furnace` view stops
    /// resolving once the entity is gone.
    ///
    /// A destroyed storage box spills its contents as a loot bag at the
    /// box's position, so breaking one open is looting, not deletion.
    pub(super) fn destroy_deployed_entity(&mut self, id: DeployedEntityId) {
        let Some(removed) = self.remove_deployed_entity_tracked(id) else {
            return;
        };
        self.spill_container_contents(removed);
        self.refresh_structural_stability();
    }

    /// Drop a removed entity's stored items (storage box slots, furnace
    /// fuel + smelt slots) as a loot bag where it stood. No-op for kinds
    /// with no contents; breaking a container open is looting, not
    /// deletion.
    pub(super) fn spill_container_contents(&mut self, removed: DeployedEntity) {
        let mut spilled: Vec<crate::protocol::ItemStack> = Vec::new();
        if let Some(storage) = removed.storage {
            spilled.extend(storage.slots.into_iter().flatten());
        }
        if let Some(furnace) = removed.furnace {
            spilled.extend(furnace.fuel);
            spilled.extend(furnace.items.into_iter().flatten());
        }
        if !spilled.is_empty() {
            self.spawn_loot_bag(removed.position, removed.yaw, spilled);
        }
    }

    /// True when `position` is a spot a free deployable can stand on:
    /// the world floor, or exactly on the walkable top of a building
    /// platform (foundation or ceiling) whose cell contains the XZ.
    /// Cells are axis-aligned because building yaw is quarter-turn
    /// snapped.
    pub(super) fn valid_deployable_surface(&self, position: Vec3Net) -> bool {
        if position.y.abs() <= 0.25 {
            return true;
        }
        let half = crate::building::FOUNDATION_SIZE_M / 2.0;
        self.deployed_entities.values().any(|entity| {
            let DeployableKind::Building { piece, .. } = entity.kind else {
                return false;
            };
            let Some(top) = crate::building::platform_top_offset(piece) else {
                return false;
            };
            (entity.position.y + top - position.y).abs() <= 0.05
                && (entity.position.x - position.x).abs() <= half
                && (entity.position.z - position.z).abs() <= half
        })
    }

    /// The bookkeeping half of a destroy: drop the entity from the
    /// authoritative map, the chunk tracker, and any open-furnace
    /// pointers. No stability recompute; callers that want the
    /// structural collapse go through [`Self::destroy_deployed_entity`].
    pub(super) fn remove_deployed_entity_tracked(
        &mut self,
        id: DeployedEntityId,
    ) -> Option<DeployedEntity> {
        let removed = self.remove_deployed_entity(id)?;
        self.chunk_manager.untrack_deployed_entity(id);
        // Loot bags resting on the removed piece fall to the next support
        // instead of floating where the floor used to be.
        self.unsettle_loot_bags_on(&removed);
        // Clear any client's open-furnace / open-storage-box pointer at
        // this id so they don't keep operating a destroyed entity.
        for client in self.clients.values_mut() {
            if client.open_furnace == Some(id) {
                client.open_furnace = None;
            }
            if client.open_container == Some(super::loot_bag::OpenContainer::StorageBox(id)) {
                client.open_container = None;
            }
        }
        Some(removed)
    }

    /// True when any of `blocks` overlaps an existing deployable's solid
    /// boxes. `skip` exempts one entity id (a door's parent doorway).
    /// Wall-like building blocks legitimately touch (and corner-overlap)
    /// their foundation and each other, so building-vs-building pairs are
    /// resolved by socket occupancy in the building module instead of by
    /// this box test; see `apply_place_building_command`.
    pub(super) fn any_deployable_overlaps(
        &self,
        blocks: &[WorldBlock],
        skip: Option<DeployedEntityId>,
    ) -> bool {
        for existing in self.deployed_entities.values() {
            if Some(existing.id) == skip {
                continue;
            }
            for existing_block in existing.resolved_collider_blocks() {
                for candidate in blocks {
                    if blocks_overlap(*candidate, existing_block) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// True when the player has any placed deployable in range that
    /// satisfies `station`. Used by `enqueue_craft` to gate recipes
    /// behind workbench/furnace presence.
    pub(super) fn station_in_range(&self, client_id: ClientId, station: RecipeStation) -> bool {
        if matches!(station, RecipeStation::None) {
            return true;
        }
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let player_pos = client.controller.position;
        for entity in self.deployed_entities.values() {
            if !station.satisfied_by(entity.kind) {
                continue;
            }
            let Some(profile) = item_definition(&entity.item_id).and_then(|def| def.deployable)
            else {
                continue;
            };
            if player_pos.within_horizontal_range(entity.position, profile.station_radius) {
                return true;
            }
        }
        false
    }

    /// Build the load-time map from a list of persisted deployable
    /// entries. Drops entries whose item id no longer resolves so a
    /// retired item type doesn't crash the load.
    pub(super) fn restore_deployed_entities(
        persisted: Vec<crate::save::PersistedDeployedEntity>,
    ) -> HashMap<DeployedEntityId, DeployedEntity> {
        persisted
            .into_iter()
            .filter_map(|p| {
                let item_id = crate::items::intern_item_id(&p.item_id);
                item_definition(&item_id)?;
                let furnace = p.furnace.map(super::furnace::FurnaceState::from_persisted);
                let torch = p.torch.map(super::torch::TorchState::from_persisted);
                let door = p.door.map(super::door::DoorState::from_persisted);
                let storage = match p.kind {
                    DeployableKind::StorageBox { tier } => Some(
                        p.storage
                            .map(|s| super::storage_box::StorageBoxState::from_persisted(s, tier))
                            .unwrap_or_else(|| super::storage_box::StorageBoxState::new(tier)),
                    ),
                    _ => None,
                };
                Some((
                    p.id,
                    DeployedEntity {
                        id: p.id,
                        item_id,
                        kind: p.kind,
                        position: p.position,
                        yaw: p.yaw,
                        health: p.health,
                        max_health: p.max_health,
                        owner: p.owner,
                        furnace,
                        placed_at_tick: p.placed_at_tick,
                        door,
                        label: p.label,
                        // Recomputed by the post-load stability refresh.
                        stability: 100,
                        storage,
                        torch,
                    },
                ))
            })
            .collect()
    }

    /// Convert the live map back into save records. Order is sorted by
    /// id so save files diff cleanly across reloads.
    pub(super) fn persisted_deployed_entities(&self) -> Vec<crate::save::PersistedDeployedEntity> {
        let mut entries: Vec<_> = self
            .deployed_entities
            .values()
            .map(|entity| crate::save::PersistedDeployedEntity {
                id: entity.id,
                item_id: entity.item_id.as_ref().to_owned(),
                kind: entity.kind,
                position: entity.position,
                yaw: entity.yaw,
                health: entity.health,
                max_health: entity.max_health,
                owner: entity.owner,
                furnace: entity.furnace.as_ref().map(|f| f.to_persisted()),
                placed_at_tick: entity.placed_at_tick,
                door: entity.door.as_ref().map(|door| door.to_persisted()),
                label: entity.label.clone(),
                storage: entity.storage.as_ref().map(|s| s.to_persisted()),
                torch: entity.torch.as_ref().map(|t| t.to_persisted()),
            })
            .collect();
        entries.sort_by_key(|entry| entry.id);
        entries
    }
}

fn place_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}

/// True when `position` is within `range` (horizontally) of the nearest
/// point on any of `blocks`. Range checks against placed structures
/// measure to the surface, not the centre, so wide pieces (a 3 m
/// foundation slab) are hittable from any side the player can reach.
pub(super) fn within_horizontal_range_of_blocks(
    position: Vec3Net,
    blocks: &[WorldBlock],
    range: f32,
) -> bool {
    blocks.iter().any(|block| {
        let min = block.min();
        let max = block.max();
        let dx = position.x - position.x.clamp(min.x, max.x);
        let dz = position.z - position.z.clamp(min.z, max.z);
        (dx * dx + dz * dz).sqrt() <= range
    })
}

/// Penetration below this doesn't count as overlap; touching faces (a
/// deployable standing exactly on a foundation top) plus f32 rounding
/// must not read as collision.
const OVERLAP_EPSILON_M: f32 = 0.001;

fn blocks_overlap(a: WorldBlock, b: WorldBlock) -> bool {
    let a_min = a.min();
    let a_max = a.max();
    let b_min = b.min();
    let b_max = b.max();
    a_min.x + OVERLAP_EPSILON_M < b_max.x
        && a_max.x > b_min.x + OVERLAP_EPSILON_M
        && a_min.y + OVERLAP_EPSILON_M < b_max.y
        && a_max.y > b_min.y + OVERLAP_EPSILON_M
        && a_min.z + OVERLAP_EPSILON_M < b_max.z
        && a_max.z > b_min.z + OVERLAP_EPSILON_M
}

#[cfg(test)]
mod tests;
