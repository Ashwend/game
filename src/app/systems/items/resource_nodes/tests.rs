use super::pop_in::{POP_IN_DURATION_SECS, ease_out_cubic, pop_in_transform};
use super::*;
use crate::resources::{COAL_NODE_ID, IRON_NODE_ID, ResourceNodeModel, SULFUR_NODE_ID};

#[test]
fn pop_in_starts_below_floor_and_settles_to_base_transform() {
    let base = Transform::from_xyz(3.0, 0.0, -2.0).with_scale(Vec3::ONE);

    // At t=0 the node is fully buried, the very first frame the
    // animation runs the entity should be at the deepest point.
    let at_start = pop_in_transform(base, 0.0);
    assert!(
        at_start.translation.y < base.translation.y - 0.4,
        "pop-in should start well below the floor, got {at_start:?}"
    );
    assert!(at_start.scale.length() <= base.scale.length() + 1e-3);

    // Mid-curve the node is on its way up and slightly above unit
    // scale (the overshoot pulse), but still below its final y.
    let mid = pop_in_transform(base, POP_IN_DURATION_SECS * 0.6);
    assert!(mid.translation.y > at_start.translation.y);
    assert!(mid.translation.y < base.translation.y);

    // Past the window the result snaps exactly back to the base
    // transform so subsequent snapshot updates take over cleanly.
    let after = pop_in_transform(base, POP_IN_DURATION_SECS + 1.0);
    assert_eq!(after.translation, base.translation);
    assert_eq!(after.scale, base.scale);
}

#[test]
fn ore_transform_matches_spawn_y_so_rock_sits_on_ground() {
    // The ore meshes have their lowest vertex at local y=0, so the
    // transform must not raise them above the floor.
    for ore_id in [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID] {
        let position = Vec3Net::new(2.0, 0.0, -3.0);
        let definition = crate::resources::resource_node_definition(ore_id).unwrap();
        let transform = resource_node_transform_at(7, position, 0.0, definition.model);
        assert_eq!(
            transform.translation.y, position.y,
            "{ore_id} mesh must sit at the spawn y (no floating offset)"
        );
    }
}

#[test]
fn ease_out_cubic_spans_zero_to_one_monotonically() {
    assert_eq!(ease_out_cubic(0.0), 0.0);
    assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-6);
    // Eased value leads a linear ramp in the middle (ease-out).
    assert!(ease_out_cubic(0.5) > 0.5);
    // Clamped below 0 and above 1.
    assert_eq!(ease_out_cubic(-1.0), 0.0);
    assert!((ease_out_cubic(2.0) - 1.0).abs() < 1e-6);
}

#[test]
fn pop_in_overshoots_above_unit_scale_mid_curve() {
    let base = Transform::from_xyz(0.0, 0.0, 0.0).with_scale(Vec3::ONE);
    // Just past the overshoot peak (raw ~0.7) the node briefly scales
    // beyond its base size before settling.
    let mid = pop_in_transform(base, POP_IN_DURATION_SECS * 0.65);
    assert!(mid.scale.length() > base.scale.length());
}

#[test]
fn tree_transform_carries_bounded_uniform_jitter_on_the_ground() {
    let position = Vec3Net::new(1.0, 0.0, 2.0);
    let transform = resource_node_transform_at(11, position, 0.5, ResourceNodeModel::PineTreeLarge);
    // Trees scale uniformly (no squash), within the ±12% jitter band.
    assert_eq!(transform.scale.x, transform.scale.y);
    assert_eq!(transform.scale.y, transform.scale.z);
    assert!((0.88..=1.12).contains(&transform.scale.x));
    assert_eq!(transform.translation.y, position.y);
    // Yaw is applied as a rotation about Y.
    let expected = Quat::from_rotation_y(0.5);
    assert!(transform.rotation.dot(expected).abs() > 1.0 - 1e-5);
}

#[test]
fn size_jitter_is_deterministic_per_node_id_and_varies_between_ids() {
    let position = Vec3Net::new(0.0, 0.0, 0.0);
    let model = ResourceNodeModel::PineTreeMedium;
    let first = resource_node_transform_at(3, position, 0.0, model);
    let again = resource_node_transform_at(3, position, 0.0, model);
    assert_eq!(
        first.scale, again.scale,
        "same node id must always produce the same size"
    );
    let other = resource_node_transform_at(4, position, 0.0, model);
    assert_ne!(
        first.scale, other.scale,
        "different node ids should land on different sizes"
    );
}

#[test]
fn ore_models_carry_per_model_scale_shaping() {
    let position = Vec3Net::new(0.0, 0.0, 0.0);
    let iron = resource_node_transform_at(5, position, 0.0, ResourceNodeModel::IronOre);
    let coal = resource_node_transform_at(5, position, 0.0, ResourceNodeModel::CoalOre);
    // Iron has a distinct non-uniform shape on top of the shared jitter;
    // coal stays uniform.
    assert_ne!(iron.scale, coal.scale);
    assert_eq!(coal.scale.x, coal.scale.y);
    // Iron's x/y ratio survives the uniform jitter multiply.
    let ratio = iron.scale.x / iron.scale.y;
    assert!((ratio - 1.1 / 1.05).abs() < 1e-5);
}
