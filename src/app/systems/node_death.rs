use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;

use crate::{
    app::audio::{PlaySound, SoundId},
    app::scene::{ImpactEffectAssets, ToonMaterial, tree_mesh_height},
    app::state::ImpactEffectKind,
    items::ToolKind,
    protocol::ResourceNodeId,
    resources::ResourceNodeModel,
    util::hash::hashed_unit,
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
/// Half-width of the per-tree random spread applied to the fall direction.
/// Trees lead away from the player, but each one is rotated by a hashed offset
/// in `[-MAX, MAX]` so a cleared grove doesn't have every trunk lying in
/// lockstep. ~0.6 rad ≈ 34° keeps the trunk clearly pointing away.
const MAX_FALL_JITTER_RADIANS: f32 = 0.6;

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
    material: Handle<ToonMaterial>,
    /// Cloned canopy material, present for live trees so the solid foliage child
    /// fades out together with the trunk. `None` for dead snags (single mesh, no
    /// canopy).
    canopy_material: Option<Handle<ToonMaterial>>,
    landed_age: Option<f32>,
    landing_kick_fired: bool,
    landing_chips_fired: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_node_death(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    materials: &mut Assets<ToonMaterial>,
    camera_kick: &mut CameraImpactKick,
    node_id: ResourceNodeId,
    model: ResourceNodeModel,
    transform: Transform,
    mesh: Handle<Mesh>,
    material: Handle<ToonMaterial>,
    canopy: Option<(Handle<Mesh>, Handle<ToonMaterial>)>,
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
            canopy,
            player_position,
        );
    } else if model.is_crude() {
        let _ = (mesh, material, canopy);
        spawn_crude_pickup_burst(commands, impact_assets, node_id, model, transform);
    } else {
        let _ = (mesh, material, canopy);
        spawn_ore_shatter(
            commands,
            impact_assets,
            play,
            camera_kick,
            node_id,
            transform,
            player_position,
        );
    }
}

/// Small upward burst for the "pickup completed" frame of a crude node
/// (branch pile / surface stone / hay tuft). No camera kick, the player
/// is just snatching something off the ground, and the per-model
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
    materials: &mut Assets<ToonMaterial>,
    node_id: ResourceNodeId,
    model: ResourceNodeModel,
    transform: Transform,
    mesh: Handle<Mesh>,
    source_material: Handle<ToonMaterial>,
    canopy: Option<(Handle<Mesh>, Handle<ToonMaterial>)>,
    player_position: Option<Vec3>,
) {
    let Some(base_height) = tree_mesh_height(model) else {
        return;
    };

    // Fire the crash audio at the same instant the felling component
    // gets created. The clip's audible climax arrives ~0.6 s in, which
    // sits naturally with the pendulum-fall reaching horizontal for a
    // typical tree, tall trees fall a little slower so the crash lands
    // slightly early, short trees a little late, but the lead-in noise
    // hides the small mismatch.
    play.write(PlaySound::at(SoundId::TreeFall, transform.translation));

    let fall_direction =
        compute_horizontal_fall_direction(player_position, transform.translation, node_id);
    let fall_axis = fall_axis_from_direction(fall_direction);

    // Clone the source material so we can drive this falling tree's `fade`
    // without touching the shared material every other resource node uses.
    // Keep the clone at `fade == 1.0` (opaque) for now: an opaque trunk draws in
    // the opaque phase and correctly depth-occludes the detail grass (which
    // renders in the transparent phase but still writes depth). Only the
    // end-of-life fade drops `fade` below 1.0, which flips the clone into the
    // Blend pass (see `ToonMaterial::alpha_mode` + `apply_fade_out`). Forcing
    // Blend up front put the still-solid, upright trunk into the transparent
    // phase, so grass behind it punched through on the first frame of the fall.
    let fade_material = match materials.get(&source_material) {
        Some(source) => materials.add(source.clone()),
        None => source_material,
    };

    // Clone the canopy material too, so the falling tree fades its own foliage
    // without dragging every other tree's shared canopy `ToonMaterial` along.
    let canopy = canopy.map(|(canopy_mesh, canopy_source)| {
        let canopy_material = match materials.get(&canopy_source) {
            Some(source) => materials.add(source.clone()),
            None => canopy_source,
        };
        (canopy_mesh, canopy_material)
    });
    let canopy_material = canopy.as_ref().map(|(_, m)| m.clone());

    let mut felling = commands.spawn((
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
            canopy_material,
            landed_age: None,
            landing_kick_fired: false,
            landing_chips_fired: false,
        },
        Mesh3d(mesh),
        MeshMaterial3d(fade_material),
        transform,
        Visibility::Visible,
    ));
    // The canopy rides as a child at local identity so it falls + fades with the
    // trunk (the parent's animated transform rotates it about the same base pivot).
    // It casts shadows like the standing tree's canopy, so the tree's shadow stays
    // consistent through the fall instead of snapping to a thin trunk-only shadow.
    if let Some((canopy_mesh, canopy_material)) = canopy {
        felling.with_children(|parent| {
            parent.spawn((
                Mesh3d(canopy_mesh),
                MeshMaterial3d(canopy_material),
                Transform::default(),
                Visibility::Visible,
            ));
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_ore_shatter(
    commands: &mut Commands,
    impact_assets: &ImpactEffectAssets,
    play: &mut MessageWriter<PlaySound>,
    camera_kick: &mut CameraImpactKick,
    node_id: ResourceNodeId,
    transform: Transform,
    player_position: Option<Vec3>,
) {
    // The death effect is purely particles, the rock visibly breaks apart
    // and falls to the ground. Heavy gravity inside the shatter burst keeps
    // chunks from sailing through the air like an explosion.
    let burst_anchor = transform.translation + Vec3::Y * ORE_BURST_HEIGHT;
    let _ = player_position;

    spawn_ore_shatter_burst(
        commands,
        impact_assets,
        burst_anchor,
        (node_id as u32).wrapping_mul(0xC2B2AE35),
        1.0,
    );

    // The "that's the whole node" finisher: a distinct break sound so the
    // player knows to stop swinging without watching the storage tooltip,
    // the same role the node-finished pop plays in Rust. Trees get the
    // equivalent signal from `TreeFall`.
    play.write(PlaySound::at(SoundId::OreNodeBreak, burst_anchor));

    camera_kick.trigger(ToolKind::Pickaxe);
}

pub(crate) fn tick_felling_trees_system(
    mut commands: Commands,
    time: Res<Time>,
    impact_assets: Res<ImpactEffectAssets>,
    mut materials: ResMut<Assets<ToonMaterial>>,
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

/// Tiny kinematic overshoot at landing, a damped oscillation around
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
        // trunk is actually lying, regardless of which way it fell. The
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
    materials: &mut Assets<ToonMaterial>,
    entity: Entity,
    tree: &FellingTree,
    since_land: f32,
) {
    if since_land < TREE_LANDED_HOLD {
        return;
    }
    let fade_t = ((since_land - TREE_LANDED_HOLD) / TREE_FADE_DURATION).clamp(0.0, 1.0);
    let alpha = (1.0 - fade_t).clamp(0.0, 1.0);
    // Lower `fade` on the cloned trunk material. Crossing below 1.0 flips it into
    // the Blend pass (`ToonMaterial::alpha_mode`); up to this point it was opaque
    // so it occluded the grass correctly, and by now it's lying flat on the
    // ground dissolving, so the brief transparent-phase sorting is not visible.
    if let Some(material) = materials.get_mut(&tree.material) {
        material.fade = alpha;
    }
    // Dissolve the solid canopy alongside the trunk: the same `fade` ramp fades
    // the whole leafy mass smoothly to nothing.
    if let Some(canopy_material) = tree.canopy_material.as_ref()
        && let Some(material) = materials.get_mut(canopy_material)
    {
        material.fade = alpha;
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
            // Lead away from the player, then rotate by a per-node hashed
            // offset so neighbouring trunks don't all topple in lockstep and a
            // trunk rarely lands straight back over where the player stands.
            let jitter = (hashed_unit(node_id as u32) - 0.5) * 2.0 * MAX_FALL_JITTER_RADIANS;
            return (Quat::from_rotation_y(jitter) * away.normalize()).normalize_or_zero();
        }
    }

    // Deterministic fallback so each tree always falls the same way even if
    // the player isn't recorded (e.g. snapshot mid-load). Uses the node id
    // as the seed.
    let angle = (node_id as f32) * 0.137 + 0.31;
    Vec3::new(angle.cos(), 0.0, angle.sin()).normalize_or_zero()
}

/// The rotation axis that tips a trunk *along* `fall_direction`. Rotating the
/// trunk's local +Y (its length) by a positive angle around `Y × fall_direction`
/// swings the top toward `fall_direction`; the reverse order (`fall_direction ×
/// Y`) tips it the opposite way, which is what made felled trees appear to fall
/// back toward the player. Falls back to `Vec3::X` for a degenerate (vertical)
/// direction, which never happens for the horizontal directions we feed it.
fn fall_axis_from_direction(fall_direction: Vec3) -> Vec3 {
    let axis = Vec3::Y.cross(fall_direction).normalize_or_zero();
    if axis.length_squared() < f32::EPSILON {
        Vec3::X
    } else {
        axis
    }
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
        // The away direction is +X; the per-node jitter rotates it within a
        // ±MAX cone, so the X component stays at least cos(MAX).
        assert!(direction.x >= MAX_FALL_JITTER_RADIANS.cos() - 1e-3);
        assert!(direction.length() > 0.99);
        assert!(direction.length() < 1.01);
        assert!(direction.y.abs() < 1e-6);
    }

    #[test]
    fn fall_direction_jitter_varies_per_node_but_stays_in_cone() {
        let player = Vec3::ZERO;
        let tree = Vec3::new(4.0, 0.0, 0.0);
        let a = compute_horizontal_fall_direction(Some(player), tree, 1);
        let b = compute_horizontal_fall_direction(Some(player), tree, 2);
        // Different nodes get different spread...
        assert!(a.distance(b) > 1e-3);
        // ...but every one still leads away from the player (+X half-plane).
        for dir in [a, b] {
            assert!(dir.x > 0.0);
        }
    }

    #[test]
    fn felled_trunk_tips_away_from_the_player() {
        // Regression for the cross-product sign: the trunk must topple along
        // `fall_direction` (away from the player at the origin), not back over
        // it. Reproduce the production transform math and check the trunk's
        // length axis (+Y) leans in the same horizontal direction we chose.
        let player = Vec3::ZERO;
        let tree = Vec3::new(4.0, 0.0, 0.0);
        let dir = compute_horizontal_fall_direction(Some(player), tree, 1);
        let axis = fall_axis_from_direction(dir);

        // Part-way through the fall the trunk's length (+Y) tips over.
        let leaning = Quat::from_axis_angle(axis, 0.6) * Vec3::Y;
        let horizontal = Vec3::new(leaning.x, 0.0, leaning.z);

        // It leans away from the player (the tree sits at +X relative to them)
        // and specifically along the chosen fall direction.
        assert!(horizontal.x > 0.0, "trunk leaned back toward the player");
        assert!(horizontal.normalize().dot(dir) > 0.99);
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
            canopy_material: None,
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
            canopy_material: None,
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

        // Past the overshoot duration it's a no-op, leaves the pose alone.
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
