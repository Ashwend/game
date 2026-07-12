//! Dropped-item pickup geometry: the view-ray cone the client highlights
//! with, the lenient distance-only reach the server accepts a pickup with,
//! and the shared look-direction helper both use.

use crate::protocol::{DroppedWorldItem, Vec3Net};

pub const PICKUP_RANGE: f32 = 3.4;
const PICKUP_RAY_RADIUS: f32 = 0.58;
const PICKUP_ANCHOR_HEIGHT: f32 = 0.28;

pub fn look_forward(yaw: f32, pitch: f32) -> Vec3Net {
    let pitch_cos = pitch.cos();
    Vec3Net::new(-yaw.sin() * pitch_cos, pitch.sin(), -yaw.cos() * pitch_cos).normalize_or_zero()
}

pub fn pickup_anchor(item: &DroppedWorldItem) -> Vec3Net {
    pickup_anchor_from_position(item.position)
}

pub fn pickup_anchor_from_position(position: Vec3Net) -> Vec3Net {
    position.plus(Vec3Net::new(0.0, PICKUP_ANCHOR_HEIGHT, 0.0))
}

pub fn pickup_score(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> Option<f32> {
    pickup_score_at_position(eye, yaw, pitch, item.position)
}

/// Projection-along-ray distance from the eye to the pickup anchor at
/// `position`. `None` when the point is outside the swept pickup
/// cylinder. Same math as [`pickup_score`] but reads the position
/// directly so callers iterating replicated `DroppedItemTransform`
/// don't need to materialise a `DroppedWorldItem`.
pub fn pickup_score_at_position(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    position: Vec3Net,
) -> Option<f32> {
    let anchor = pickup_anchor_from_position(position);
    let to_item = anchor.minus(eye);
    // Cheap distance cull before the trig in `look_forward`. Anything outside
    // the swept cylinder is unreachable; the bound stays conservative so it
    // never rejects a candidate the ray test would have accepted.
    let max_reach_sq = (PICKUP_RANGE + PICKUP_RAY_RADIUS).powi(2);
    if to_item.length_squared() > max_reach_sq {
        return None;
    }

    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let projection = to_item.dot(forward);
    if !(0.0..=PICKUP_RANGE).contains(&projection) {
        return None;
    }

    let closest = eye.plus(forward.scale(projection));
    let lateral = anchor.minus(closest);
    if lateral.length_squared() > PICKUP_RAY_RADIUS * PICKUP_RAY_RADIUS {
        return None;
    }

    Some(projection)
}

pub fn can_pick_up(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> bool {
    pickup_score(eye, yaw, pitch, item).is_some()
}

/// Lenient, distance-only reach test the *server* uses to accept a pickup,
/// instead of re-running the strict view-ray [`can_pick_up`]. The client
/// already chose this exact item with the view ray and only sends a command
/// for a target it accepted; by the time that command arrives the player has
/// usually moved or turned, so the strict cone test would reject a legitimate
/// pickup and force a visible client rollback. `slack` is the extra reach
/// beyond [`PICKUP_RANGE`] that absorbs the movement-prediction delta (see
/// `PICKUP_SERVER_REACH_SLACK_M` in `game_balance`). Look direction is
/// intentionally ignored here.
pub fn within_pickup_reach(eye: Vec3Net, item_position: Vec3Net, slack: f32) -> bool {
    let anchor = pickup_anchor_from_position(item_position);
    let reach = PICKUP_RANGE + slack.max(0.0);
    anchor.minus(eye).length_squared() <= reach * reach
}

pub fn best_pickup_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    items: impl Iterator<Item = &'a DroppedWorldItem>,
) -> Option<&'a DroppedWorldItem> {
    items
        .filter_map(|item| pickup_score(eye, yaw, pitch, item).map(|score| (score, item)))
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::COAL_ID;
    use crate::protocol::{ItemStack, QuatNet};

    #[test]
    fn pickup_target_uses_view_ray_and_range() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(COAL_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        assert!(can_pick_up(eye, 0.0, -0.16, &item));
        assert!(!can_pick_up(eye, std::f32::consts::PI, -0.16, &item));
    }

    #[test]
    fn server_pickup_reach_is_lenient_and_distance_only() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(COAL_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        // Looking the other way fails the strict client test (used for
        // highlighting) but the server's distance-only check still accepts it,
        // so a player who turned away after pressing E isn't rolled back.
        assert!(!can_pick_up(eye, std::f32::consts::PI, 0.0, &item));
        assert!(within_pickup_reach(eye, item.position, 1.5));

        // Beyond PICKUP_RANGE + slack it's still rejected, the leniency is
        // bounded, not unlimited reach.
        let far = Vec3Net::new(0.0, 0.6, -10.0);
        assert!(!within_pickup_reach(far, item.position, 1.5));
    }
}
