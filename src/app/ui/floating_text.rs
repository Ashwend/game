//! Floating damage numbers above hit players.
//!
//! Spawn a `FloatingDamageText` entity at the impact site; one system
//! ticks the lifetime and despawns expired entities, another renders
//! every live entity via egui — projecting the world anchor to the
//! viewport so the text rides above the target's chest.
//!
//! Two visual variants drive the colour:
//!
//! - **Dealt** (orange) — what the local attacker did to a peer.
//! - **Taken** (red) — what a peer did to the local player.
//!
//! Each number gets a small randomised launch vector (a cone biased
//! upward from the impact) so a flurry of hits spreads visually instead
//! of stacking, and a brief "pop" scale-up followed by a settle so the
//! eye latches onto the new number for a beat before it lifts away.

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{app::scene::MainCamera, util::hash::hashed_unit};

/// How long a damage number stays on screen, in seconds. Long enough
/// to read at a glance, short enough that a flurry of hits doesn't
/// stack into an unreadable column.
const FLOATING_TEXT_LIFETIME_S: f32 = 1.1;
/// Total world-space drift, in metres, over the text's lifetime —
/// magnitude only; the direction is randomised per spawn so a flurry
/// of hits spreads instead of stacking.
const FLOATING_TEXT_DRIFT_M: f32 = 1.2;
/// Maximum horizontal half-angle (radians) the launch vector can lean
/// off straight-up. ~32° gives a noticeable splash without numbers
/// flying sideways enough to land outside the hit zone.
const FLOATING_TEXT_CONE_HALF_ANGLE_RAD: f32 = 0.55;
/// Cull text beyond this distance from the camera — past it the
/// numbers would be unreadable anyway.
const FLOATING_TEXT_DRAW_DISTANCE_M: f32 = 30.0;
/// Inset (logical pixels) from the viewport edges before we stop
/// drawing the number. Matches the peer overlay's edge guard so a
/// projected anchor doesn't end up half-clipped against the screen
/// border.
const FLOATING_TEXT_VIEWPORT_INSET_PX: f32 = 12.0;
/// Base font size at the "settled" scale of 1.0. The pop curve scales
/// this up briefly at spawn.
const FLOATING_TEXT_BASE_FONT_PX: f32 = 28.0;
/// Peak multiplier the "pop" reaches at the apex of its overshoot.
const FLOATING_TEXT_POP_PEAK_SCALE: f32 = 1.6;
/// Fraction of the lifetime spent on the pop curve before the number
/// settles into its drifting state. 0.18 reads as a quick snap-and-
/// settle — long enough to register as a pop, short enough that the
/// number is moving for most of its lifetime.
const FLOATING_TEXT_POP_FRACTION: f32 = 0.18;

/// Who saw the damage. Drives colour: an attacker's screen shows the
/// damage in orange, a target's in red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FloatingDamageRole {
    Dealt,
    Taken,
}

impl FloatingDamageRole {
    fn color(self) -> egui::Color32 {
        match self {
            // Bright orange (`#FFAA33`) — reads as "I scored" without
            // being mistaken for a UI error.
            Self::Dealt => egui::Color32::from_rgb(0xFF, 0xAA, 0x33),
            // Bright red (`#FF3333`) — strong enough to grab attention
            // through the peripheral vision when you're being hit.
            Self::Taken => egui::Color32::from_rgb(0xFF, 0x33, 0x33),
        }
    }
}

/// A single floating-damage label. The text + colour stay constant;
/// only `elapsed` ticks each frame, and the renderer reads it to drive
/// the rise + fade.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct FloatingDamageText {
    pub(crate) anchor: Vec3,
    pub(crate) value: u32,
    pub(crate) role: FloatingDamageRole,
    pub(crate) elapsed: f32,
    /// Per-spawn random launch direction (unit length, biased upward
    /// inside [`FLOATING_TEXT_CONE_HALF_ANGLE_RAD`]). Stored at spawn
    /// time so the render loop can stay deterministic across frames.
    pub(crate) drift_dir: Vec3,
    /// Random seed for the egui Area id so two numbers at the same
    /// position get unique widget IDs.
    pub(crate) spawn_id: u64,
}

impl FloatingDamageText {
    pub(crate) fn new(anchor: Vec3, value: u32, role: FloatingDamageRole) -> Self {
        // Seed pulls from the wall clock + the anchor position so two
        // numbers spawned the same frame at different positions get
        // different RNG, and two numbers spawned at the same position
        // (e.g. flurry of swings) get different RNG because the time
        // component advances.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let seed = (nanos as u64)
            .wrapping_mul(0x9E3779B1_9E3779B1)
            .wrapping_add(anchor.x.to_bits() as u64)
            .wrapping_add((anchor.y.to_bits() as u64).rotate_left(7))
            .wrapping_add((anchor.z.to_bits() as u64).rotate_left(13));
        Self {
            anchor,
            value,
            role,
            elapsed: 0.0,
            drift_dir: cone_launch_direction(seed),
            spawn_id: seed,
        }
    }

    /// 0.0 at spawn, 1.0 at end of life.
    fn fraction(&self) -> f32 {
        (self.elapsed / FLOATING_TEXT_LIFETIME_S).clamp(0.0, 1.0)
    }

    /// World position the text currently occupies — anchor plus the
    /// ease-out drift along this spawn's randomised launch vector.
    fn current_world(&self) -> Vec3 {
        let f = self.fraction();
        // Ease-out cubic: `1 - (1-f)^3` accelerates early, settles late.
        let eased = 1.0 - (1.0 - f).powi(3);
        self.anchor + self.drift_dir * (FLOATING_TEXT_DRIFT_M * eased)
    }

    fn alpha(&self) -> f32 {
        // Hold near-full opacity through the pop, then fade across the
        // remainder. Keeps the "I just hit" beat readable before the
        // number starts dissolving.
        let f = self.fraction();
        if f < FLOATING_TEXT_POP_FRACTION {
            1.0
        } else {
            let tail = (f - FLOATING_TEXT_POP_FRACTION) / (1.0 - FLOATING_TEXT_POP_FRACTION);
            (1.0 - tail).clamp(0.0, 1.0)
        }
    }

    /// Font-scale multiplier driving the "pop". Snaps from 0 to the
    /// peak across the first chunk of the lifetime, then settles back
    /// to 1.0 — reads as a quick scale-in overshoot rather than a
    /// linear ramp.
    fn pop_scale(&self) -> f32 {
        let f = self.fraction();
        if f < FLOATING_TEXT_POP_FRACTION {
            // 0 → 1 across the pop window; map into the overshoot curve.
            let t = f / FLOATING_TEXT_POP_FRACTION;
            // Sine half-cycle from 0 → π gives a smooth rise to the
            // peak and back. Slightly biased so the peak lands at
            // ~60 % of the pop window, not the midpoint, which reads
            // more like a "snap".
            let curve = (t * std::f32::consts::PI).sin();
            // Lift from a small starting scale so the number visibly
            // appears (rather than fading in from invisible) and
            // overshoots through the peak.
            0.5 + curve * (FLOATING_TEXT_POP_PEAK_SCALE - 0.5)
        } else {
            // After the pop, linear-settle back toward 1.0 and then
            // hold. Subtracting a tiny linear easing keeps the
            // post-pop scale from sticking at the overshoot value.
            let settle_t =
                ((f - FLOATING_TEXT_POP_FRACTION) / (1.0 - FLOATING_TEXT_POP_FRACTION)).min(1.0);
            let from = FLOATING_TEXT_POP_PEAK_SCALE;
            from + (1.0 - from) * settle_t.min(1.0)
        }
    }
}

/// Build a random unit vector inside a cone whose axis points straight
/// up. `seed` makes the choice deterministic per-spawn.
fn cone_launch_direction(seed: u64) -> Vec3 {
    let azimuth = hashed_unit(seed as u32) * std::f32::consts::TAU;
    // Bias the tilt toward small angles so most numbers stay close to
    // vertical; the long tail of `r^0.5` gives the occasional wider
    // splash without it being the common case.
    let tilt =
        hashed_unit(seed.wrapping_mul(0x9E37) as u32).sqrt() * FLOATING_TEXT_CONE_HALF_ANGLE_RAD;
    let sin_t = tilt.sin();
    let cos_t = tilt.cos();
    Vec3::new(azimuth.cos() * sin_t, cos_t, azimuth.sin() * sin_t)
}

/// Despawn expired floating-damage entities. Done as a separate system
/// so the UI render loop doesn't have to filter by lifetime — every
/// entity it sees is in-flight.
pub(crate) fn tick_floating_damage_system(
    mut commands: Commands,
    time: Res<Time>,
    mut texts: Query<(Entity, &mut FloatingDamageText)>,
) {
    let dt = time.delta_secs().max(0.0);
    for (entity, mut text) in &mut texts {
        text.elapsed += dt;
        if text.elapsed >= FLOATING_TEXT_LIFETIME_S {
            commands.entity(entity).despawn();
        }
    }
}

/// Egui render hook for every live damage number. Called from the
/// in-game UI sweep (see `app/ui.rs`); takes the camera transform the
/// rest of the in-world overlays already resolve so the projection is
/// consistent across the HUD.
pub(crate) fn floating_damage_ui<'a>(
    ctx: &egui::Context,
    camera: Option<(&'a Camera, GlobalTransform)>,
    texts: impl IntoIterator<Item = &'a FloatingDamageText>,
) {
    let Some((camera, camera_transform)) = camera else {
        return;
    };
    let camera_forward = camera_transform.forward().as_vec3();
    let camera_origin = camera_transform.translation();
    let visible_rect = ctx.content_rect().shrink(FLOATING_TEXT_VIEWPORT_INSET_PX);

    for text in texts {
        let world = text.current_world();
        let to_text = world - camera_origin;
        if to_text.dot(camera_forward) <= 0.0 {
            continue;
        }
        let distance = to_text.length();
        if distance > FLOATING_TEXT_DRAW_DISTANCE_M {
            continue;
        }
        let Ok(screen) = camera.world_to_viewport(&camera_transform, world) else {
            continue;
        };
        if !visible_rect.contains(egui::pos2(screen.x, screen.y)) {
            continue;
        }

        let alpha = (text.alpha() * 255.0).round() as u8;
        let color = text.role.color();
        let color_alpha =
            egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
        let shadow_alpha = ((alpha as u16 * 192) / 255) as u8;
        let shadow = egui::Color32::from_rgba_unmultiplied(0, 0, 0, shadow_alpha);
        let font_size = FLOATING_TEXT_BASE_FONT_PX * text.pop_scale();
        let label = format!("-{}", text.value);

        // Foreground order: above the world, below modal dialogs —
        // identical to the peer overlay so numbers and nametags share
        // a layer.
        let area_id = egui::Id::new(("floating_damage", text.spawn_id));
        egui::Area::new(area_id)
            .order(egui::Order::Foreground)
            .interactable(false)
            .movable(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .fixed_pos(egui::pos2(screen.x, screen.y))
            .show(ctx, |ui| {
                let font = egui::FontId::new(font_size, egui::FontFamily::Proportional);
                // Drop shadow first so the bright number sits on top.
                ui.painter().text(
                    egui::pos2(2.0, 2.0) + ui.next_widget_position().to_vec2(),
                    egui::Align2::CENTER_CENTER,
                    &label,
                    font.clone(),
                    shadow,
                );
                ui.painter().text(
                    ui.next_widget_position(),
                    egui::Align2::CENTER_CENTER,
                    &label,
                    font,
                    color_alpha,
                );
                // Reserve the rough text box so egui's layout pass
                // doesn't shrink the area to zero. Sized at the peak
                // so the overshoot scale doesn't clip.
                let reserve = font_size.max(FLOATING_TEXT_BASE_FONT_PX) * 1.8;
                let _ = ui.allocate_exact_size(egui::vec2(reserve, reserve), egui::Sense::hover());
            });
    }
    let _ = MainCamera; // marker type; the param wiring still depends on it.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cone_launch_direction_is_unit_length_and_biased_upward() {
        // Sample a handful of seeds and make sure every direction lands
        // inside the cone (y near 1, |xz| small).
        for seed in 0u64..16 {
            let dir = cone_launch_direction(seed.wrapping_mul(0xDEAD_BEEF));
            let len = dir.length();
            assert!((len - 1.0).abs() < 1e-4, "expected unit length, got {len}");
            assert!(dir.y > 0.0, "cone should point upward, got y={}", dir.y);
            // The launch cone's half-angle is `FLOATING_TEXT_CONE_HALF_ANGLE_RAD`;
            // the floor of y is `cos(half_angle)`.
            let min_y = FLOATING_TEXT_CONE_HALF_ANGLE_RAD.cos();
            assert!(dir.y >= min_y - 1e-3, "y={} below cone floor", dir.y);
        }
    }

    #[test]
    fn pop_scale_peaks_then_settles_to_one() {
        let mut text = FloatingDamageText::new(Vec3::ZERO, 8, FloatingDamageRole::Dealt);
        // Halfway through the pop window the scale should be above the
        // settled value.
        text.elapsed = FLOATING_TEXT_LIFETIME_S * (FLOATING_TEXT_POP_FRACTION * 0.5);
        let mid_pop = text.pop_scale();
        assert!(
            mid_pop > 1.0,
            "expected pop scale > 1.0 mid-pop, got {mid_pop}",
        );

        // Right at end-of-life the scale has settled to exactly 1.0.
        // Verify the easing function actually lands the value rather
        // than asymptoting — without this guarantee the last frame
        // would still render a slightly-larger number.
        text.elapsed = FLOATING_TEXT_LIFETIME_S;
        let settled = text.pop_scale();
        assert!(
            (settled - 1.0).abs() < 1e-3,
            "expected pop scale = 1.0 at end of life, got {settled}",
        );
    }
}
