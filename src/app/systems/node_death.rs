use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::{
    app::scene::{ImpactEffectAssets, tree_mesh_height},
    app::state::ImpactEffectKind,
    items::ToolKind,
    protocol::ResourceNodeId,
    resources::ResourceNodeModel,
};

use super::{
    CameraImpactKick,
    effects::{spawn_impact_burst, spawn_ore_shatter_burst},
};

// Tree felling tuning.
const TREE_FALL_GRAVITY: f32 = 14.0;
const TREE_INITIAL_ANGLE: f32 = 0.04;
const TREE_INITIAL_PUSH: f32 = 0.55;
const TREE_OVERSHOOT_AMPLITUDE: f32 = 0.06;
const TREE_OVERSHOOT_DURATION: f32 = 0.28;
const TREE_LANDED_HOLD: f32 = 0.55;
const TREE_FADE_DURATION: f32 = 1.05;
const TREE_GROUND_LIFT: f32 = 0.16;

// Ore shatter tuning.
const ORE_BURST_HEIGHT: f32 = 0.35;

#[derive(Component, Debug)]
pub(crate) struct FellingTree {
    age: f32,
    angle: f32,
    angular_velocity: f32,
    fall_axis: Vec3,
    lever_length: f32,
    pivot: Vec3,
    initial_rotation: Quat,
    initial_scale: Vec3,
    material: Handle<StandardMaterial>,
    landed_age: Option<f32>,
    landing_kick_fired: bool,
    landing_chips_fired: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_node_death(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    materials: &mut Assets<StandardMaterial>,
    camera_kick: &mut CameraImpactKick,
    node_id: ResourceNodeId,
    model: ResourceNodeModel,
    transform: Transform,
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
    player_position: Option<Vec3>,
) {
    if model.is_tree() {
        spawn_tree_felling(
            commands,
            materials,
            node_id,
            model,
            transform,
            mesh,
            material,
            player_position,
        );
    } else {
        let _ = (mesh, material);
        spawn_ore_shatter(
            commands,
            impact_assets,
            camera_kick,
            node_id,
            transform,
            player_position,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_tree_felling(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    node_id: ResourceNodeId,
    model: ResourceNodeModel,
    transform: Transform,
    mesh: Handle<Mesh>,
    source_material: Handle<StandardMaterial>,
    player_position: Option<Vec3>,
) {
    let Some(base_height) = tree_mesh_height(model) else {
        return;
    };

    let fall_direction =
        compute_horizontal_fall_direction(player_position, transform.translation, node_id);
    let fall_axis = fall_direction.cross(Vec3::Y).normalize_or_zero();
    let fall_axis = if fall_axis.length_squared() < f32::EPSILON {
        Vec3::X
    } else {
        fall_axis
    };

    // Clone the source material so we can drive this falling tree's alpha
    // without touching the shared material that other resource nodes use.
    // AlphaMode::Blend lets us smoothly fade the trunk out at the end of
    // the death animation.
    let fade_material = match materials.get(&source_material) {
        Some(source) => {
            let mut clone = source.clone();
            clone.alpha_mode = AlphaMode::Blend;
            materials.add(clone)
        }
        None => source_material,
    };

    commands.spawn((
        Name::new(format!("Felling Tree {node_id}")),
        FellingTree {
            age: 0.0,
            angle: TREE_INITIAL_ANGLE,
            angular_velocity: TREE_INITIAL_PUSH,
            fall_axis,
            lever_length: (base_height * transform.scale.y).max(0.4),
            pivot: transform.translation,
            initial_rotation: transform.rotation,
            initial_scale: transform.scale,
            material: fade_material.clone(),
            landed_age: None,
            landing_kick_fired: false,
            landing_chips_fired: false,
        },
        Mesh3d(mesh),
        MeshMaterial3d(fade_material),
        transform,
        Visibility::Visible,
    ));
}

fn spawn_ore_shatter(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    camera_kick: &mut CameraImpactKick,
    node_id: ResourceNodeId,
    transform: Transform,
    player_position: Option<Vec3>,
) {
    // The death effect is purely particles — the rock visibly breaks apart
    // and falls to the ground. Heavy gravity inside the shatter burst keeps
    // chunks from sailing through the air like an explosion.
    let burst_anchor = transform.translation + Vec3::Y * ORE_BURST_HEIGHT;
    let _ = player_position;

    spawn_ore_shatter_burst(
        commands,
        impact_assets,
        burst_anchor,
        (node_id as u32).wrapping_mul(0xC2B2AE35),
    );

    camera_kick.trigger(ToolKind::Pickaxe);
}

pub(crate) fn tick_felling_trees_system(
    mut commands: Commands,
    time: Res<Time>,
    impact_assets: Res<ImpactEffectAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut camera_kick: ResMut<CameraImpactKick>,
    mut trees: Query<(Entity, &mut Transform, &mut FellingTree)>,
) {
    let dt = time.delta_secs().clamp(0.0, 0.05);
    if dt == 0.0 {
        return;
    }

    for (entity, mut transform, mut tree) in &mut trees {
        tree.age += dt;

        if tree.landed_age.is_none() {
            step_pendulum(&mut tree, dt);
        }
        apply_base_transform(&tree, &mut transform);

        if let Some(landed_at) = tree.landed_age {
            let since_land = tree.age - landed_at;
            apply_landing_overshoot(&tree, &mut transform, since_land);
            fire_landing_feedback(
                &mut commands,
                &impact_assets,
                &mut camera_kick,
                entity,
                &mut tree,
                &transform,
            );
            transform.scale = tree.initial_scale;
            apply_fade_out(&mut commands, &mut materials, entity, &tree, since_land);
        } else {
            transform.scale = tree.initial_scale;
        }
    }
}

/// Pendulum integration: α = (3g / (2L)) · sin(θ). Heavier (taller) trees
/// naturally fall more slowly thanks to the longer lever. Clamps to 90° on
/// contact and records the landing time so subsequent phases can fire.
fn step_pendulum(tree: &mut FellingTree, dt: f32) {
    let alpha = (3.0 * TREE_FALL_GRAVITY / (2.0 * tree.lever_length)) * tree.angle.sin();
    tree.angular_velocity += alpha * dt;
    tree.angle += tree.angular_velocity * dt;

    if tree.angle >= FRAC_PI_2 {
        tree.angle = FRAC_PI_2;
        tree.angular_velocity = 0.0;
        tree.landed_age = Some(tree.age);
    }
}

/// Apply rotation around the fall axis plus a small ground lift, so the
/// trunk rests on the ground rather than half-buried in it after rotation.
fn apply_base_transform(tree: &FellingTree, transform: &mut Transform) {
    let lift = (1.0 - tree.angle.cos()) * TREE_GROUND_LIFT;
    transform.rotation = Quat::from_axis_angle(tree.fall_axis, tree.angle) * tree.initial_rotation;
    transform.translation = tree.pivot + Vec3::Y * lift;
}

/// Tiny kinematic overshoot at landing — a damped oscillation around
/// horizontal that reads as the trunk bouncing off the ground. Doesn't
/// affect `angular_velocity` afterwards; once the overshoot window closes
/// the trunk sits at exactly 90°.
fn apply_landing_overshoot(tree: &FellingTree, transform: &mut Transform, since_land: f32) {
    if since_land >= TREE_OVERSHOOT_DURATION {
        return;
    }
    let t = since_land / TREE_OVERSHOOT_DURATION;
    let damp = 1.0 - t;
    let phase = t * std::f32::consts::PI * 2.4;
    let overshoot = phase.sin() * TREE_OVERSHOOT_AMPLITUDE * damp;
    transform.rotation =
        Quat::from_axis_angle(tree.fall_axis, FRAC_PI_2 + overshoot) * tree.initial_rotation;
}

/// One-shot feedback on the frame the trunk hits the ground: a camera kick
/// and a chip burst at the far end of the lying trunk. Guarded by `tree`'s
/// own latched flags so the second-and-later frames after landing are
/// silent.
fn fire_landing_feedback(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    camera_kick: &mut CameraImpactKick,
    entity: Entity,
    tree: &mut FellingTree,
    transform: &Transform,
) {
    if !tree.landing_kick_fired {
        tree.landing_kick_fired = true;
        camera_kick.trigger(ToolKind::Pickaxe);
    }
    if !tree.landing_chips_fired {
        tree.landing_chips_fired = true;
        // Spawn the chips at the centre of the lying trunk in world space.
        // The mesh's +Y axis is the trunk's length direction, so rotating
        // it by the current world rotation gives us whichever way the
        // trunk is actually lying — regardless of which way it fell. The
        // small Y offset lifts the burst up to roughly the top surface of
        // the lying trunk so the chips read as flying off it.
        let lying_direction = transform.rotation * Vec3::Y;
        let landing_point =
            transform.translation + lying_direction * (tree.lever_length * 0.5) + Vec3::Y * 0.15;
        spawn_impact_burst(
            commands,
            impact_assets,
            ImpactEffectKind::WoodChips,
            landing_point,
            Vec3::Y,
            entity.to_bits() as u32,
            2.0,
        );
    }
}

/// Hold at full opacity for a beat, then alpha-fade the trunk out. The
/// trunk stays at full size so it reads as the wood dissolving rather than
/// crumpling into the ground. Despawns when fully transparent.
fn apply_fade_out(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    entity: Entity,
    tree: &FellingTree,
    since_land: f32,
) {
    if since_land < TREE_LANDED_HOLD {
        return;
    }
    let fade_t = ((since_land - TREE_LANDED_HOLD) / TREE_FADE_DURATION).clamp(0.0, 1.0);
    let alpha = (1.0 - fade_t).clamp(0.0, 1.0);
    if let Some(material) = materials.get_mut(&tree.material) {
        material.base_color.set_alpha(alpha);
    }
    if fade_t >= 1.0 {
        commands.entity(entity).despawn();
    }
}

fn compute_horizontal_fall_direction(
    player_position: Option<Vec3>,
    tree_position: Vec3,
    node_id: ResourceNodeId,
) -> Vec3 {
    if let Some(player) = player_position {
        let away = Vec3::new(tree_position.x - player.x, 0.0, tree_position.z - player.z);
        if away.length_squared() > 0.01 {
            return away.normalize();
        }
    }

    // Deterministic fallback so each tree always falls the same way even if
    // the player isn't recorded (e.g. snapshot mid-load). Uses the node id
    // as the seed.
    let angle = (node_id as f32) * 0.137 + 0.31;
    Vec3::new(angle.cos(), 0.0, angle.sin()).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fall_direction_points_away_from_player() {
        let direction = compute_horizontal_fall_direction(
            Some(Vec3::new(0.0, 0.0, 0.0)),
            Vec3::new(4.0, 0.0, 0.0),
            1,
        );
        assert!(direction.x > 0.9);
        assert!(direction.length() > 0.99);
        assert!(direction.length() < 1.01);
        assert!(direction.y.abs() < 1e-6);
    }

    #[test]
    fn fall_direction_falls_back_to_deterministic_when_player_missing() {
        let direction = compute_horizontal_fall_direction(None, Vec3::ZERO, 7);
        assert!(direction.length() > 0.99);
        assert!(direction.length() < 1.01);
        assert!(direction.y.abs() < 1e-6);
    }

    #[test]
    fn felling_tree_pendulum_lands_after_a_reasonable_duration() {
        let mut tree = FellingTree {
            age: 0.0,
            angle: TREE_INITIAL_ANGLE,
            angular_velocity: TREE_INITIAL_PUSH,
            fall_axis: Vec3::X,
            lever_length: 2.5,
            pivot: Vec3::ZERO,
            initial_rotation: Quat::IDENTITY,
            initial_scale: Vec3::ONE,
            material: Handle::default(),
            landed_age: None,
            landing_kick_fired: false,
            landing_chips_fired: false,
        };

        let dt = 1.0 / 60.0;
        let mut elapsed = 0.0;
        while tree.angle < FRAC_PI_2 && elapsed < 5.0 {
            let alpha = (3.0 * TREE_FALL_GRAVITY / (2.0 * tree.lever_length)) * tree.angle.sin();
            tree.angular_velocity += alpha * dt;
            tree.angle += tree.angular_velocity * dt;
            elapsed += dt;
        }

        assert!(elapsed > 0.5, "the tree should not fall instantly");
        assert!(elapsed < 2.0, "the tree should land in under two seconds");
    }
}
