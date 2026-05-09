use std::collections::HashMap;

use rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderSet, ImpulseJointSet, IntegrationParameters,
    IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline, RigidBodyBuilder,
    RigidBodyHandle, RigidBodySet, Vector,
};

use crate::{
    protocol::{DroppedItemId, DroppedWorldItem, QuatNet, Vec3Net},
    world::WorldData,
};

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

#[derive(Debug)]
pub(super) struct DroppedItemBody {
    pub(super) item: DroppedWorldItem,
    pub(super) body_handle: RigidBodyHandle,
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
