use std::collections::HashMap;

use rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderSet, ImpulseJointSet, IntegrationParameters,
    IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, Vector,
};

use crate::{
    items::{ItemId, stack_limit},
    protocol::{DroppedItemId, DroppedWorldItem, QuatNet, SERVER_TICK_RATE_HZ, Vec3Net},
    world::WorldData,
};

use super::GameServer;

pub(super) const DROPPED_ITEM_RADIUS: f32 = 0.1;
const DROPPED_ITEM_SPIN_RADIUS: f32 = 0.18;
const DROPPED_ITEM_FLOOR_HALF_HEIGHT: f32 = 0.05;
const DROPPED_ITEM_RESTITUTION: f32 = 0.04;
const DROPPED_ITEM_FRICTION: f32 = 1.65;
const DROPPED_ITEM_LINEAR_DAMPING: f32 = 1.25;
const DROPPED_ITEM_ANGULAR_DAMPING: f32 = 4.0;
const DROPPED_ITEM_MASS: f32 = 1.2;
const DROPPED_ITEM_GRAVITY_Y: f32 = -18.0;
const DROPPED_ITEM_MAX_SIMULATION_DELTA: f32 = 0.1;
const DROPPED_ITEM_MAX_SIMULATION_STEP: f32 = 1.0 / 120.0;
pub(super) const DROPPED_ITEM_MERGE_INTERVAL_TICKS: u64 = 5;
pub(super) const DROPPED_ITEM_MERGE_RADIUS: f32 = 1.0;

/// How long a dropped item is allowed to sit in the world before the server
/// despawns it to keep long-running sessions from accumulating stale stacks.
/// Three minutes is generous enough that a player can travel a short distance
/// and come back for what they dropped, but tight enough that abandoned loot
/// doesn't bloat the snapshot indefinitely.
pub(super) const DROPPED_ITEM_LIFETIME_SECONDS: f32 = 180.0;
pub(super) const DROPPED_ITEM_LIFETIME_TICKS: u64 =
    (DROPPED_ITEM_LIFETIME_SECONDS * SERVER_TICK_RATE_HZ) as u64;
/// Cadence of the lifetime sweep. One pass per second is plenty, the
/// timeout has second-scale granularity and the sweep is O(N).
pub(super) const DROPPED_ITEM_CLEANUP_INTERVAL_TICKS: u64 = SERVER_TICK_RATE_HZ as u64;

#[derive(Debug)]
pub(super) struct DroppedItemBody {
    pub(super) item: DroppedWorldItem,
    pub(super) body_handle: RigidBodyHandle,
    /// Tick at which this body entered the world. In-memory only, items
    /// reloaded from a save are stamped with the load-time tick so a player
    /// returning after a long absence isn't greeted by a wave of instant
    /// despawns.
    pub(super) spawn_tick: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct DroppedItemPhysicsBody {
    pub(super) body_handle: RigidBodyHandle,
}

pub(super) struct DroppedItemPhysics {
    pipeline: PhysicsPipeline,
    integration_parameters: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
}

impl std::fmt::Debug for DroppedItemPhysics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DroppedItemPhysics")
            .field("body_count", &self.bodies.len())
            .field("collider_count", &self.colliders.len())
            .finish_non_exhaustive()
    }
}

impl DroppedItemPhysics {
    pub(super) fn new(world: &WorldData) -> Self {
        let mut physics = Self {
            pipeline: PhysicsPipeline::new(),
            integration_parameters: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
        };
        physics.spawn_static_world(world);
        physics
    }

    fn spawn_static_world(&mut self, world: &WorldData) {
        let floor_body = self
            .bodies
            .insert(RigidBodyBuilder::fixed().translation(Vector::new(
                0.0,
                -DROPPED_ITEM_FLOOR_HALF_HEIGHT,
                0.0,
            )));
        self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(
                world.floor_size * 0.5,
                DROPPED_ITEM_FLOOR_HALF_HEIGHT,
                world.floor_size * 0.5,
            )
            .friction(DROPPED_ITEM_FRICTION)
            .restitution(DROPPED_ITEM_RESTITUTION),
            floor_body,
            &mut self.bodies,
        );

        for block in &world.blocks {
            let body = self
                .bodies
                .insert(RigidBodyBuilder::fixed().translation(Vector::new(
                    block.center.x,
                    block.center.y,
                    block.center.z,
                )));
            self.colliders.insert_with_parent(
                ColliderBuilder::cuboid(
                    block.half_extents.x,
                    block.half_extents.y,
                    block.half_extents.z,
                )
                .friction(DROPPED_ITEM_FRICTION)
                .restitution(DROPPED_ITEM_RESTITUTION),
                body,
                &mut self.bodies,
            );
        }
    }

    pub(super) fn spawn_body(
        &mut self,
        position: Vec3Net,
        velocity: Vec3Net,
        yaw: f32,
    ) -> DroppedItemPhysicsBody {
        let body = RigidBodyBuilder::dynamic()
            .translation(to_rapier_vector(position))
            .rotation(Vector::new(0.0, yaw, 0.0))
            .linvel(to_rapier_vector(velocity))
            .angvel(initial_drop_angular_velocity(velocity))
            .linear_damping(DROPPED_ITEM_LINEAR_DAMPING)
            .angular_damping(DROPPED_ITEM_ANGULAR_DAMPING)
            .ccd_enabled(true)
            .build();
        let body_handle = self.bodies.insert(body);
        self.colliders.insert_with_parent(
            ColliderBuilder::ball(DROPPED_ITEM_RADIUS)
                .mass(DROPPED_ITEM_MASS)
                .friction(DROPPED_ITEM_FRICTION)
                .restitution(DROPPED_ITEM_RESTITUTION),
            body_handle,
            &mut self.bodies,
        );
        DroppedItemPhysicsBody { body_handle }
    }

    pub(super) fn remove_body(&mut self, handle: RigidBodyHandle) {
        self.bodies.remove(
            handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
    }

    pub(super) fn step(
        &mut self,
        delta_seconds: f32,
        dropped_items: &mut HashMap<DroppedItemId, DroppedItemBody>,
    ) {
        let mut remaining = if delta_seconds.is_finite() {
            delta_seconds.clamp(0.0, DROPPED_ITEM_MAX_SIMULATION_DELTA)
        } else {
            0.0
        };

        while remaining > 0.0 {
            let step = remaining.min(DROPPED_ITEM_MAX_SIMULATION_STEP);
            self.integration_parameters.dt = step;
            self.pipeline.step(
                Vector::new(0.0, DROPPED_ITEM_GRAVITY_Y, 0.0),
                &self.integration_parameters,
                &mut self.islands,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.bodies,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                &mut self.ccd_solver,
                &(),
                &(),
            );
            remaining -= step;
        }

        for body in dropped_items.values_mut() {
            let Some(rigid_body) = self.bodies.get(body.body_handle) else {
                continue;
            };
            let translation = rigid_body.translation();
            let rotation = rigid_body.rotation();
            body.item.position = Vec3Net::new(translation.x, translation.y, translation.z);
            body.item.rotation = QuatNet::new(rotation.x, rotation.y, rotation.z, rotation.w);

            let linvel = rigid_body.linvel();
            let horizontal_speed_sq = linvel.x.mul_add(linvel.x, linvel.z * linvel.z);
            if horizontal_speed_sq > 0.0025 {
                body.item.yaw = (-linvel.x).atan2(-linvel.z);
            }
        }
    }
}

pub(super) fn nearby_dropped_item_pairs(
    dropped_items: &HashMap<DroppedItemId, DroppedItemBody>,
) -> Vec<(DroppedItemId, DroppedItemId)> {
    let mut items = dropped_items
        .iter()
        .map(|(item_id, body)| (*item_id, body.item.position))
        .collect::<Vec<_>>();
    items.sort_by_key(|(item_id, _)| *item_id);

    let merge_radius_sq = DROPPED_ITEM_MERGE_RADIUS * DROPPED_ITEM_MERGE_RADIUS;
    let mut pairs = Vec::new();
    for first_index in 0..items.len() {
        for second_index in first_index + 1..items.len() {
            let (first_id, first_position) = items[first_index];
            let (second_id, second_position) = items[second_index];
            if first_position.minus(second_position).length_squared() <= merge_radius_sq {
                pairs.push((first_id, second_id));
            }
        }
    }
    pairs
}

pub(super) fn yaw_rotation(yaw: f32) -> QuatNet {
    let half_yaw = yaw * 0.5;
    QuatNet::new(0.0, half_yaw.sin(), 0.0, half_yaw.cos())
}

fn to_rapier_vector(value: Vec3Net) -> Vector {
    Vector::new(value.x, value.y, value.z)
}

fn initial_drop_angular_velocity(velocity: Vec3Net) -> Vector {
    Vector::new(
        velocity.z / DROPPED_ITEM_SPIN_RADIUS,
        0.0,
        -velocity.x / DROPPED_ITEM_SPIN_RADIUS,
    )
}

impl GameServer {
    /// Remove dropped items whose age has exceeded
    /// [`DROPPED_ITEM_LIFETIME_TICKS`]. Runs from the routine cleanup tick
    /// (see [`DROPPED_ITEM_CLEANUP_INTERVAL_TICKS`]) so the cost is amortized
    /// to ~once a second regardless of how many items are on the ground.
    /// Clients learn about the removal through the next snapshot, the same
    /// path used when an item is picked up or fully merged.
    pub(super) fn despawn_aging_dropped_items(&mut self) {
        let current_tick = self.tick;
        let expired: Vec<DroppedItemId> = self
            .dropped_items
            .iter()
            .filter_map(|(id, body)| {
                let age = current_tick.saturating_sub(body.spawn_tick);
                (age >= DROPPED_ITEM_LIFETIME_TICKS).then_some(*id)
            })
            .collect();
        for id in expired {
            if let Some(body) = self.dropped_items.remove(&id) {
                self.dropped_item_physics.remove_body(body.body_handle);
                self.chunk_manager.untrack_dropped_item(id);
            }
        }
    }

    pub(super) fn merge_nearby_dropped_items(&mut self) -> Vec<(ItemId, u16)> {
        // Returns the interned `ItemId` (not a fresh `String`) so the
        // resulting `ServerMessage::ItemMerged` doesn't allocate per merge.
        let mut merges = Vec::new();
        for (first_id, second_id) in nearby_dropped_item_pairs(&self.dropped_items) {
            if let Some(merge) = self.merge_dropped_item_pair(first_id, second_id) {
                merges.push(merge);
            }
        }
        merges
    }

    fn merge_dropped_item_pair(
        &mut self,
        first_id: DroppedItemId,
        second_id: DroppedItemId,
    ) -> Option<(ItemId, u16)> {
        // Compute the merge up-front from immutable reads so we never have
        // to remove-then-reinsert when a validation step fails. Once `moved`
        // is finalised the mutation is straight-through.
        let (target_id, source_id) = self.merge_target_and_source(first_id, second_id)?;
        let (limit, target_quantity, source_quantity) = {
            let target = self.dropped_items.get(&target_id)?;
            let source = self.dropped_items.get(&source_id)?;
            let limit = stack_limit(&target.item.stack.item_id)?;
            (
                limit,
                target.item.stack.quantity,
                source.item.stack.quantity,
            )
        };
        let moved = limit.saturating_sub(target_quantity).min(source_quantity);
        if moved == 0 {
            return None;
        }
        // Refuse partial merges. If the source can't be fully absorbed (e.g.
        // 100 + 8 with a 100 stack limit), moving anything just swaps which
        // body is "full" and which is "small", both still exist and the
        // pair is back in `nearby_dropped_item_pairs` on the very next merge
        // tick, oscillating forever. Leaving the smaller stack alone until
        // there's room for all of it removes the flip while still letting
        // genuinely combinable pairs (50 + 50, 99 + 1, …) merge in one shot.
        if moved < source_quantity {
            return None;
        }

        let item_id = {
            let target = self.dropped_items.get_mut(&target_id)?;
            target.item.stack.quantity += moved;
            target.item.stack.item_id.clone()
        };

        let drain_source = {
            let source = self.dropped_items.get_mut(&source_id)?;
            source.item.stack.quantity -= moved;
            source.item.stack.quantity == 0
        };
        if drain_source && let Some(body) = self.dropped_items.remove(&source_id) {
            self.dropped_item_physics.remove_body(body.body_handle);
            self.chunk_manager.untrack_dropped_item(source_id);
        }

        Some((item_id, moved))
    }

    fn merge_target_and_source(
        &self,
        first_id: DroppedItemId,
        second_id: DroppedItemId,
    ) -> Option<(DroppedItemId, DroppedItemId)> {
        let first = self.dropped_items.get(&first_id)?;
        let second = self.dropped_items.get(&second_id)?;
        if first.item.stack.item_id != second.item.stack.item_id {
            return None;
        }

        let limit = stack_limit(&first.item.stack.item_id)?;
        let first_room = limit.saturating_sub(first.item.stack.quantity);
        let second_room = limit.saturating_sub(second.item.stack.quantity);
        match (first_room > 0, second_room > 0) {
            (false, false) => None,
            (true, false) => Some((first_id, second_id)),
            (false, true) => Some((second_id, first_id)),
            (true, true) if first.item.stack.quantity >= second.item.stack.quantity => {
                Some((first_id, second_id))
            }
            (true, true) => Some((second_id, first_id)),
        }
    }
}
