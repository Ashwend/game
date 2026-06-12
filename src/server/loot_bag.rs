//! Server-authoritative loot bag state + open/close/move commands.
//!
//! A loot bag is the container spawned at a dead player's feet, it
//! holds every item the corpse was carrying so a killer can scoop up
//! the kill in one stop instead of running a vacuum over a pile of
//! individual `DroppedWorldItem`s. The bag persists until either:
//!   - all slots are empty AND the holder closes the UI, or
//!   - a future cleanup pass (lifetime-based) sweeps it.
//!
//! Command shape mirrors `FurnaceCommand` because the UI semantics
//! are identical (open one container, drag stacks between it and the
//! player). The bag has no smelt loop, no fuel slot, no active flag.

use crate::{
    items::normalize_stack,
    protocol::{
        ClientId, ContainerViewKind, DeployedEntityId, ItemStack, LOOT_BAG_SLOT_COUNT,
        LootBagCommand, LootBagId, LootBagSlotRef, OpenLootBagView, PlayerInventoryState, Vec3Net,
    },
    server::{GameServer, ServerEnvelope},
};

mod slots;

use slots::{ContainerSlots, move_within, quick_transfer_within, reply_warning};

/// What a player currently has open in the loot-transfer UI. Both kinds drive
/// the same `OpenLootBagView` on the wire and the same `LootBagCommand`
/// handlers; they differ only in where the "container" slots live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenContainer {
    /// A world loot bag (death drop), by id.
    LootBag(LootBagId),
    /// A logged-out sleeping player's *live* inventory, by their client id. The
    /// looter reads and writes the sleeper's own slots directly, so looting is
    /// non-destructive: nothing is copied, closing leaves whatever wasn't taken
    /// on the body, and an empty body still opens (it just shows empty).
    Sleeper(ClientId),
    /// A placed storage box deployable, by entity id. Opened through
    /// `ClientMessage::OpenStorageBox` (see `super::storage_box`); the
    /// slots live on the `DeployedEntity` and persist with the world.
    StorageBox(DeployedEntityId),
}

/// Max range, in metres, at which a player can open / interact with
/// a loot bag. Loosened from the swing range so a kill that knocks
/// the corpse a step or two doesn't put the loot out of reach.
pub(crate) use crate::game_balance::LOOT_BAG_INTERACT_RANGE_M;

/// Vertical offset above the dead player's feet where the bag
/// spawns. Roughly waist-height so the bag falls naturally instead of
/// materialising on the ground.
const BAG_SPAWN_HEIGHT_M: f32 = 1.0;
/// Initial downward velocity, in m/s, applied at spawn. A small
/// upward kick from gravity-only integration looks lifeless; giving
/// the bag a tiny initial drop reads as "it slumps off the corpse".
const BAG_SPAWN_VERTICAL_VELOCITY: f32 = -0.4;
/// Gravity applied to settling bags. Matches the controller's gravity
/// so the visible fall matches the player's frame of reference.
const BAG_GRAVITY: f32 = 18.0;
/// Height of a resting bag above its support surface (the world floor
/// or a building platform's top). Slightly above the surface so the
/// visual mesh's lower face doesn't z-fight with it.
const BAG_RESTING_Y: f32 = 0.05;

/// Authoritative record of a loot bag in the world. Stored in
/// `GameServer::loot_bags` keyed by `LootBagId`. Fields are crate-
/// visible because the net-host mirror reads them when building the
/// per-tick replication snapshot.
#[derive(Debug, Clone)]
pub struct LootBag {
    pub(crate) id: LootBagId,
    pub(crate) position: Vec3Net,
    pub(crate) yaw: f32,
    pub(crate) slots: Vec<Option<ItemStack>>,
    /// Tick the bag was created on. The client uses death-time as a
    /// "I just killed them" handle for UI cues (loot glints, etc.)
    /// in the future; today this is just bookkeeping.
    #[expect(dead_code, reason = "bookkeeping for future loot-glint / kill-cue UI")]
    pub(crate) spawn_tick: u64,
    /// Vertical velocity for the spawn-time gravity settle. `0.0`
    /// once the bag is at rest; non-zero while the death drop is
    /// still falling from chest height. Horizontal velocity isn't
    /// tracked, bags fall straight down.
    pub(crate) velocity_y: f32,
    /// True once the bag has touched its support. Skips the
    /// per-tick integration in [`GameServer::tick_loot_bags`] for
    /// resting bags so the cost stays at O(spawned-this-tick) instead
    /// of O(every-bag-ever-spawned).
    pub(crate) resting: bool,
    /// Y the bag settles at: the highest support surface under its XZ
    /// (world floor or a building/deployable top) plus [`BAG_RESTING_Y`].
    /// Computed at spawn; recomputed when the supporting piece is
    /// destroyed (see [`GameServer::unsettle_loot_bags_on`]).
    pub(crate) rest_y: f32,
}

impl LootBag {
    pub(super) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub(super) fn to_view(&self) -> OpenLootBagView {
        OpenLootBagView {
            id: self.id,
            slots: self.slots.clone(),
            kind: ContainerViewKind::LootBag,
        }
    }
}

impl GameServer {
    /// Authoritative bag-spawn entry point. Used by the death chain
    /// in `combat.rs` to drop the corpse's items as a single bag.
    /// Empty input is allowed but pointless, the caller is expected
    /// to filter before calling. Returns the new bag id.
    pub(crate) fn spawn_loot_bag(
        &mut self,
        position: Vec3Net,
        yaw: f32,
        items: Vec<ItemStack>,
    ) -> LootBagId {
        let id = self.allocate_loot_bag_id();
        let mut slots: Vec<Option<ItemStack>> = vec![None; LOOT_BAG_SLOT_COUNT];
        for (index, stack) in items.into_iter().enumerate() {
            if index >= slots.len() {
                break;
            }
            slots[index] = normalize_stack(&stack);
        }
        // Spawn at chest height so the bag visibly falls off the
        // corpse instead of materialising on the ground. Gravity
        // ticks below pull it down to the support under it (a death
        // on an upper floor rests the bag on that floor, not the
        // ground storeys below).
        let spawn_position = Vec3Net::new(position.x, position.y + BAG_SPAWN_HEIGHT_M, position.z);
        let bag = LootBag {
            id,
            position: spawn_position,
            yaw,
            slots,
            spawn_tick: self.tick,
            velocity_y: BAG_SPAWN_VERTICAL_VELOCITY,
            resting: false,
            rest_y: self.loot_bag_rest_y(spawn_position),
        };
        self.loot_bags.insert(id, bag);
        self.chunk_manager.track_loot_bag(id, spawn_position);
        id
    }

    /// Integrate every in-flight loot bag's gravity drop by one tick.
    /// At-rest bags are skipped so the per-tick cost stays at
    /// O(spawned-this-tick) instead of O(every-bag-ever-spawned).
    pub(crate) fn tick_loot_bags(&mut self, delta_seconds: f32) {
        let dt = delta_seconds.clamp(0.0, 0.1);
        if dt <= 0.0 {
            return;
        }
        for bag in self.loot_bags.values_mut() {
            if bag.resting {
                continue;
            }
            bag.velocity_y -= BAG_GRAVITY * dt;
            bag.position.y += bag.velocity_y * dt;
            if bag.position.y <= bag.rest_y {
                bag.position.y = bag.rest_y;
                bag.velocity_y = 0.0;
                bag.resting = true;
            }
        }
    }

    /// Y a loot bag at `position`'s XZ comes to rest at: the highest
    /// solid top surface at or below it, from the world's static blocks
    /// and every placed structure, with the world floor as the fallback.
    /// Runs on death/destruction events only, so the linear scan over
    /// deployables is fine.
    fn loot_bag_rest_y(&self, position: Vec3Net) -> f32 {
        let mut rest = BAG_RESTING_Y;
        let mut consider = |block: &crate::world::WorldBlock| {
            let min = block.min();
            let max = block.max();
            if position.x < min.x || position.x > max.x || position.z < min.z || position.z > max.z
            {
                return;
            }
            if max.y <= position.y + 0.01 {
                rest = rest.max(max.y + BAG_RESTING_Y);
            }
        };
        for block in &self.world.blocks {
            consider(block);
        }
        for entity in self.deployed_entities.values() {
            for block in entity.resolved_collider_blocks() {
                consider(&block);
            }
        }
        rest
    }

    /// Re-float every bag that was resting on `removed`'s solid boxes so
    /// it falls to the next support below (the piece under it was just
    /// destroyed). Called from the deployable removal path.
    pub(super) fn unsettle_loot_bags_on(&mut self, removed: &super::deployables::DeployedEntity) {
        let blocks = removed.resolved_collider_blocks();
        if blocks.is_empty() {
            return;
        }
        let falling: Vec<(crate::protocol::LootBagId, Vec3Net)> = self
            .loot_bags
            .values()
            .filter(|bag| bag.resting)
            .filter(|bag| {
                blocks.iter().any(|block| {
                    let min = block.min();
                    let max = block.max();
                    bag.position.x >= min.x
                        && bag.position.x <= max.x
                        && bag.position.z >= min.z
                        && bag.position.z <= max.z
                        && (bag.position.y - (max.y + BAG_RESTING_Y)).abs() <= 0.05
                })
            })
            .map(|bag| (bag.id, bag.position))
            .collect();
        for (id, position) in falling {
            let rest_y = self.loot_bag_rest_y(position);
            if let Some(bag) = self.loot_bags.get_mut(&id) {
                bag.rest_y = rest_y;
                bag.resting = false;
                bag.velocity_y = 0.0;
            }
        }
    }

    pub(super) fn apply_loot_bag_command(
        &mut self,
        client_id: ClientId,
        command: LootBagCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            LootBagCommand::Open { id } => self.open_loot_bag(client_id, id),
            LootBagCommand::Close => {
                self.close_container(client_id);
                Vec::new()
            }
            LootBagCommand::Move { from, to, quantity } => {
                if !self.open_container_in_range(client_id) {
                    self.close_container(client_id);
                    return Vec::new();
                }
                self.move_container_stack(client_id, from, to, quantity)
            }
            LootBagCommand::QuickTransfer { from } => {
                if !self.open_container_in_range(client_id) {
                    self.close_container(client_id);
                    return Vec::new();
                }
                self.quick_transfer_container(client_id, from)
            }
        }
    }

    /// Loot a logged-out sleeping body by opening its *live* inventory as a
    /// container. Nothing is copied or moved: the looter reads and writes the
    /// sleeper's own slots, so closing without taking leaves the body exactly as
    /// it was, taking removes only what was grabbed, and an empty body still
    /// opens (showing empty) instead of being rejected. The body keeps whatever
    /// is left when it wakes.
    pub(super) fn apply_loot_sleeper(
        &mut self,
        looter_id: ClientId,
        target_id: ClientId,
    ) -> Vec<ServerEnvelope> {
        if looter_id == target_id {
            return Vec::new();
        }
        let Some(looter_pos) = self
            .clients
            .get(&looter_id)
            .map(|looter| looter.controller.position)
        else {
            return Vec::new();
        };
        // The target must be a logged-out, still-living sleeper within reach.
        let Some(target) = self.clients.get(&target_id) else {
            return Vec::new();
        };
        if target.online || target.lifecycle.is_dead() || target.controller.health <= 0.0 {
            return Vec::new();
        }
        let target_pos = target.controller.position;
        if !looter_pos.within_horizontal_range(target_pos, LOOT_BAG_INTERACT_RANGE_M) {
            return Vec::new();
        }

        if let Some(looter_mut) = self.clients.get_mut(&looter_id) {
            looter_mut.open_container = Some(OpenContainer::Sleeper(target_id));
        }
        Vec::new()
    }

    /// Re-validate that the client's open container is still in interact range
    /// (and, for a sleeper, still a lootable logged-out body). The UI can
    /// persist client-side while the player walks away; without this check
    /// stacks could be moved out of reach, or out of a body that just woke.
    fn open_container_in_range(&self, client_id: ClientId) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        let Some(container) = client.open_container else {
            return true;
        };
        let pos = client.controller.position;
        let (target_pos, range) = match container {
            OpenContainer::LootBag(bag_id) => match self.loot_bags.get(&bag_id) {
                Some(bag) => (bag.position, LOOT_BAG_INTERACT_RANGE_M),
                None => return false,
            },
            OpenContainer::Sleeper(sleeper_id) => match self.clients.get(&sleeper_id) {
                Some(sleeper)
                    if !sleeper.online
                        && !sleeper.lifecycle.is_dead()
                        && sleeper.controller.health > 0.0 =>
                {
                    (sleeper.controller.position, LOOT_BAG_INTERACT_RANGE_M)
                }
                _ => return false,
            },
            OpenContainer::StorageBox(entity_id) => match self.deployed_entities.get(&entity_id) {
                Some(entity) => (
                    entity.position,
                    super::storage_box::STORAGE_BOX_INTERACT_RANGE_M,
                ),
                None => return false,
            },
        };
        pos.within_horizontal_range(target_pos, range)
    }

    /// Quick membership / view helper for the player-private replication path.
    /// Resolves whichever container the client has open into the wire view.
    pub(crate) fn open_loot_bag_view_for(&self, client_id: ClientId) -> Option<OpenLootBagView> {
        match self.clients.get(&client_id)?.open_container? {
            OpenContainer::LootBag(bag_id) => self.loot_bags.get(&bag_id).map(LootBag::to_view),
            OpenContainer::Sleeper(sleeper_id) => {
                let sleeper = self.clients.get(&sleeper_id)?;
                // A body that woke, died, or left is no longer lootable; drop the
                // view so the looter's UI closes.
                if sleeper.online || sleeper.lifecycle.is_dead() {
                    return None;
                }
                Some(sleeper_inventory_view(sleeper_id, &sleeper.inventory))
            }
            OpenContainer::StorageBox(entity_id) => {
                let entity = self.deployed_entities.get(&entity_id)?;
                let storage = entity.storage.as_ref()?;
                Some(OpenLootBagView {
                    id: entity_id,
                    slots: storage.slots.clone(),
                    kind: ContainerViewKind::StorageBox,
                })
            }
        }
    }

    pub(crate) fn loot_bags_iter(&self) -> impl Iterator<Item = (LootBagId, &LootBag)> + '_ {
        self.loot_bags.iter().map(|(id, bag)| (*id, bag))
    }

    pub(crate) fn loot_bag_chunk(&self, id: LootBagId) -> Option<crate::world::ChunkCoord> {
        self.chunk_manager.loot_bag_chunk(id)
    }

    fn open_loot_bag(&mut self, client_id: ClientId, id: LootBagId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        let player_pos = client.controller.position;
        let Some(bag) = self.loot_bags.get(&id) else {
            return Vec::new();
        };
        if !player_pos.within_horizontal_range(bag.position, LOOT_BAG_INTERACT_RANGE_M) {
            return Vec::new();
        }
        if let Some(client_mut) = self.clients.get_mut(&client_id) {
            client_mut.open_container = Some(OpenContainer::LootBag(id));
        }
        Vec::new()
    }

    /// Close whatever container the client has open. A loot bag that's empty and
    /// no longer viewed by anyone is GC'd; a sleeper container needs no cleanup
    /// (nothing was spawned, the body's items live on the body).
    pub(crate) fn close_container(&mut self, client_id: ClientId) {
        // Take the open pointer out of the client record first so the
        // "is the bag empty after this player walked away from it?" check below
        // sees the up-to-date open state.
        let opened = self
            .clients
            .get_mut(&client_id)
            .and_then(|c| c.open_container.take());
        let Some(OpenContainer::LootBag(bag_id)) = opened else {
            return;
        };
        // If no other client has this bag open and it's empty, the entity is
        // just litter, clean it up.
        let still_open_elsewhere = self
            .clients
            .values()
            .any(|c| c.open_container == Some(OpenContainer::LootBag(bag_id)));
        if still_open_elsewhere {
            return;
        }
        if self
            .loot_bags
            .get(&bag_id)
            .map(LootBag::is_empty)
            .unwrap_or(false)
        {
            self.destroy_loot_bag(bag_id);
        }
    }

    /// Drop any looter's open view of `sleeper_id`. Called when the body wakes,
    /// dies, or is evicted so a stale view can't keep reading a changed body.
    pub(crate) fn close_sleeper_views(&mut self, sleeper_id: ClientId) {
        for client in self.clients.values_mut() {
            if client.open_container == Some(OpenContainer::Sleeper(sleeper_id)) {
                client.open_container = None;
            }
        }
    }

    pub(crate) fn destroy_loot_bag(&mut self, id: LootBagId) {
        if self.loot_bags.remove(&id).is_none() {
            return;
        }
        self.chunk_manager.untrack_loot_bag(id);
        // Clear any client's pointer at this bag so a stale Move
        // doesn't reach into a removed bag.
        for client in self.clients.values_mut() {
            if client.open_container == Some(OpenContainer::LootBag(id)) {
                client.open_container = None;
            }
        }
    }

    fn move_container_stack(
        &mut self,
        client_id: ClientId,
        from: LootBagSlotRef,
        to: LootBagSlotRef,
        quantity: Option<u16>,
    ) -> Vec<ServerEnvelope> {
        let Some(container) = self.clients.get(&client_id).and_then(|c| c.open_container) else {
            return Vec::new();
        };
        match container {
            OpenContainer::LootBag(bag_id) => {
                // Looter (in `clients`) and bag (in `loot_bags`) are disjoint
                // fields, so both borrow mutably at once.
                let Some(looter) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                let Some(bag) = self.loot_bags.get_mut(&bag_id) else {
                    return Vec::new();
                };
                move_within(
                    &mut looter.inventory,
                    &mut ContainerSlots::Bag(&mut bag.slots),
                    from,
                    to,
                    quantity,
                );
            }
            OpenContainer::Sleeper(sleeper_id) => {
                if sleeper_id == client_id {
                    return Vec::new();
                }
                let [looter, sleeper] = self.clients.get_disjoint_mut([&client_id, &sleeper_id]);
                let (Some(looter), Some(sleeper)) = (looter, sleeper) else {
                    return Vec::new();
                };
                move_within(
                    &mut looter.inventory,
                    &mut ContainerSlots::Sleeper(&mut sleeper.inventory),
                    from,
                    to,
                    quantity,
                );
            }
            OpenContainer::StorageBox(entity_id) => {
                // Player (in `clients`) and box (in `deployed_entities`)
                // are disjoint fields, so both borrow mutably at once.
                let Some(player) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                let Some(storage) = self
                    .deployed_entities
                    .get_mut(&entity_id)
                    .and_then(|entity| entity.storage.as_mut())
                else {
                    return Vec::new();
                };
                move_within(
                    &mut player.inventory,
                    &mut ContainerSlots::Bag(&mut storage.slots),
                    from,
                    to,
                    quantity,
                );
            }
        }
        Vec::new()
    }

    fn quick_transfer_container(
        &mut self,
        client_id: ClientId,
        from: LootBagSlotRef,
    ) -> Vec<ServerEnvelope> {
        let Some(container) = self.clients.get(&client_id).and_then(|c| c.open_container) else {
            return Vec::new();
        };
        let warning = match container {
            OpenContainer::LootBag(bag_id) => {
                let Some(looter) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                let Some(bag) = self.loot_bags.get_mut(&bag_id) else {
                    return Vec::new();
                };
                quick_transfer_within(
                    &mut looter.inventory,
                    &mut ContainerSlots::Bag(&mut bag.slots),
                    from,
                )
            }
            OpenContainer::Sleeper(sleeper_id) => {
                if sleeper_id == client_id {
                    return Vec::new();
                }
                let [looter, sleeper] = self.clients.get_disjoint_mut([&client_id, &sleeper_id]);
                let (Some(looter), Some(sleeper)) = (looter, sleeper) else {
                    return Vec::new();
                };
                quick_transfer_within(
                    &mut looter.inventory,
                    &mut ContainerSlots::Sleeper(&mut sleeper.inventory),
                    from,
                )
            }
            OpenContainer::StorageBox(entity_id) => {
                let Some(player) = self.clients.get_mut(&client_id) else {
                    return Vec::new();
                };
                let Some(storage) = self
                    .deployed_entities
                    .get_mut(&entity_id)
                    .and_then(|entity| entity.storage.as_mut())
                else {
                    return Vec::new();
                };
                quick_transfer_within(
                    &mut player.inventory,
                    &mut ContainerSlots::Bag(&mut storage.slots),
                    from,
                )
            }
        };
        match warning {
            Some(text) => reply_warning(client_id, text),
            None => Vec::new(),
        }
    }

    fn allocate_loot_bag_id(&mut self) -> LootBagId {
        let id = self.next_loot_bag_id;
        self.next_loot_bag_id = self.next_loot_bag_id.saturating_add(1);
        id
    }
}

/// Build the wire view of a sleeper's live inventory: the backpack slots
/// followed by the hotbar, laid out flat into the same
/// [`LOOT_BAG_SLOT_COUNT`]-wide grid the loot-bag UI renders. The view `id` is
/// the sleeper's client id (stable across reopens; the client treats it as an
/// opaque handle, so it never collides meaningfully with a real bag id).
fn sleeper_inventory_view(
    sleeper_id: ClientId,
    inventory: &PlayerInventoryState,
) -> OpenLootBagView {
    let mut slots: Vec<Option<ItemStack>> = Vec::with_capacity(LOOT_BAG_SLOT_COUNT);
    slots.extend(inventory.inventory_slots.iter().cloned());
    slots.extend(inventory.actionbar_slots.iter().cloned());
    OpenLootBagView {
        id: sleeper_id,
        slots,
        kind: ContainerViewKind::Sleeper,
    }
}

#[cfg(test)]
mod tests;
