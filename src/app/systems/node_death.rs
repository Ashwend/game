use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::{
    app::audio::{PlaySound, SoundId},
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
    play: &mut MessageWriter<PlaySound>,
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
            play,
            materials,
            node_id,
            model,
            transform,
            mesh,
            material,
            player_position,
        );
    } else if model.is_crude() {
        let _ = (mesh, material);
        spawn_crude_pickup_burst(commands, impact_assets, node_id, model, transform);
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

/// Small upward burst for the "pickup completed" frame of a crude node
/// (branch pile / surface stone / hay tuft). No camera kick — the player
/// is just snatching something off the ground — and the per-model
/// [`ImpactEffectKind`] gives each kind its own colour and footprint.
fn spawn_crude_pickup_burst(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    node_id: ResourceNodeId,
    model: ResourceNodeModel,
    transform: Transform,
) {
    let burst_anchor = transform.translation + Vec3::Y * 0.12;
    let kind = ImpactEffectKind::for_resource_model(model);
    spawn_impact_burst(
        commands,
        impact_assets,
        kind,
        burst_anchor,
        Vec3::Y,
        (node_id as u32).wrapping_mul(0xC2B2AE35),
        // Bump intensity a touch above 1.0 so the pickup-completed
        // burst reads as "finished" rather than identical to a per-hit
        // chip. Still scales off the small `Sticks`/`Pebbles`/`GrassBlades`
        // base counts, so we don't get a stone-shatter level explosion.
        1.5,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_tree_felling(
    commands: &mut Commands,
    play: &mut MessageWriter<PlaySound>,
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

    // Fire the crash audio at the same instant the felling component
    // gets created. The clip's audible climax arrives ~0.6 s in, which
    // sits naturally with the pendulum-fall reaching horizontal for a
    // typical tree — tall trees fall a little slower so the crash lands
    // slightly early, short trees a little late, but the lead-in noise
    // hides the small mismatch.
    play.write(PlaySound::at(SoundId::TreeFall, transform.translation));

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

    fn test_tree(lever_length: f32) -> FellingTree {
        FellingTree {
            age: 0.0,
            angle: TREE_INITIAL_ANGLE,
            angular_velocity: TREE_INITIAL_PUSH,
            fall_axis: Vec3::X,
            lever_length,
            pivot: Vec3::new(1.0, 0.0, -2.0),
            initial_rotation: Quat::IDENTITY,
            initial_scale: Vec3::ONE,
            material: Handle::default(),
            landed_age: None,
            landing_kick_fired: false,
            landing_chips_fired: false,
        }
    }

    #[test]
    fn step_pendulum_latches_landed_age_at_ninety_degrees() {
        let mut tree = test_tree(2.5);
        // Drive it well past the contact point with a big timestep.
        tree.angle = FRAC_PI_2 - 0.01;
        tree.angular_velocity = 5.0;
        step_pendulum(&mut tree, 0.1);
        assert_eq!(tree.angle, FRAC_PI_2);
        assert_eq!(tree.angular_velocity, 0.0);
        assert!(tree.landed_age.is_some());
    }

    #[test]
    fn apply_base_transform_rotates_and_lifts_off_the_pivot() {
        let mut tree = test_tree(2.5);
        tree.angle = FRAC_PI_2;
        let mut transform = Transform::IDENTITY;
        apply_base_transform(&tree, &mut transform);

        // At 90 degrees the ground lift is (1 - cos 90) * TREE_GROUND_LIFT.
        let expected_lift = TREE_GROUND_LIFT;
        assert!((transform.translation.y - (tree.pivot.y + expected_lift)).abs() < 1e-4);
        assert!((transform.translation.x - tree.pivot.x).abs() < 1e-4);
        // The trunk has rotated away from upright: its local up vector no
        // longer points along +Y.
        let up = transform.rotation * Vec3::Y;
        assert!(up.dot(Vec3::Y) < 0.5);
    }

    #[test]
    fn landing_overshoot_decays_to_nothing_after_its_window() {
        let mut tree = test_tree(2.5);
        tree.angle = FRAC_PI_2;
        let mut transform = Transform::IDENTITY;
        apply_base_transform(&tree, &mut transform);
        let settled = transform.rotation;

        // During the overshoot window the rotation differs from the settled
        // 90-degree pose (the bounce). Compare via the quaternion dot
        // product (|dot| == 1 means identical orientation).
        let mut bouncing = transform;
        apply_landing_overshoot(&tree, &mut bouncing, TREE_OVERSHOOT_DURATION * 0.25);
        assert!(bouncing.rotation.dot(settled).abs() < 1.0 - 1e-5);

        // Past the overshoot duration it's a no-op — leaves the pose alone.
        let mut after = transform;
        apply_landing_overshoot(&tree, &mut after, TREE_OVERSHOOT_DURATION + 0.1);
        assert!(after.rotation.dot(settled).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn crude_models_route_to_a_pickup_burst_not_a_fall() {
        // is_crude / is_tree partition the death-effect dispatch. Confirm the
        // model classification we branch on.
        assert!(ResourceNodeModel::BranchPile.is_crude());
        assert!(ResourceNodeModel::HayGrass.is_crude());
        assert!(ResourceNodeModel::PineTreeLarge.is_tree());
        assert!(!ResourceNodeModel::CoalOre.is_tree());
        assert!(!ResourceNodeModel::CoalOre.is_crude());
    }
}
