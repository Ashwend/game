use super::*;

#[test]
fn a_flying_arrow_aims_along_its_velocity() {
    // The arrow glb points along +Y; a flying arrow rotates so +Y runs down the
    // velocity direction. A shot travelling along -Z should point its +Y axis at -Z.
    let velocity = Vec3::new(0.0, 0.0, -35.0);
    let transform = arrow_transform(Vec3::new(1.0, 2.0, 3.0), velocity, Quat::IDENTITY);
    let aimed = transform.rotation * Vec3::Y;
    assert!(
        (aimed - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4,
        "the arrow's +Y axis aims along -Z travel, got {aimed:?}"
    );
    assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
}

#[test]
fn a_zero_velocity_arrow_keeps_its_fallback_orientation() {
    // Only a TRUE zero velocity has no direction to aim along; it keeps the
    // passed-in fallback so such an arrow holds its last orientation instead
    // of snapping to face nowhere.
    let fallback = Quat::from_rotation_x(0.7);
    let transform = arrow_transform(Vec3::new(5.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 0.0), fallback);
    assert!(
        transform.rotation.abs_diff_eq(fallback, 1e-6),
        "a zero-velocity arrow keeps its fallback orientation"
    );
}

#[test]
fn a_stuck_arrow_aims_along_its_epsilon_rest_direction() {
    // A stuck arrow's replicated velocity is a tiny epsilon along its final
    // flight direction (PROJECTILE_REST_DIR_EPSILON server-side). Even a
    // client that never saw the arrow fly must orient the shaft into the
    // impact from that direction alone: pointing stuck arrows straight up
    // (the old identity fallback) was the owner-reported bug.
    let fallback = Quat::IDENTITY;
    let rest_dir = Vec3::new(0.0, -0.6, -0.8);
    let stuck = arrow_transform(Vec3::ZERO, rest_dir * 0.01, fallback);
    let aimed = stuck.rotation * Vec3::Y;
    assert!(
        (aimed - rest_dir.normalize()).length() < 1e-4,
        "the stuck shaft aims along the epsilon rest direction, got {aimed:?}"
    );
}
