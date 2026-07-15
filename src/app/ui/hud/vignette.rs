//! Low-health vignette: the red screen-edge gradient mesh (plus the critical
//! full-screen wash) that closes in as the local player's health drains.
//! [`vignette_shape`] is the pure health-to-geometry ramp; the painter folds
//! in the time-varying pulse.

use bevy_egui::egui;

use crate::protocol::MAX_HEALTH;

/// Fraction of max health at/under which the low-health vignette begins to
/// fade in. Set high on purpose: the red should creep in as soon as you're
/// meaningfully hurt, not ambush you at death's door. The `VIGNETTE_SEVERITY_CURVE`
/// exponent is what keeps the early band nearly invisible.
const VIGNETTE_THRESHOLD: f32 = 0.75;
/// Exponent applied to the linear severity ramp. Above 1 the curve stays low
/// through the healthy end of the band and accelerates toward empty, so a
/// scratch tints the very edge while a near-death player is badly hemmed in.
const VIGNETTE_SEVERITY_CURVE: f32 = 2.2;
/// Peak edge alpha at zero health. Heavy enough to genuinely crowd vision at
/// the periphery, still short of an opaque wall.
const VIGNETTE_PEAK_ALPHA: f32 = 225.0;
/// Clear-center radius as a fraction of the screen's smaller dimension, at the
/// moment the vignette first appears. The red fully fades out by here.
const VIGNETTE_CLEAR_INSET_MAX: f32 = 0.30;
/// Clear-center radius at zero health. The gap between this and
/// [`VIGNETTE_CLEAR_INSET_MAX`] is what makes the red close *inward* as you
/// bleed out, shrinking how much of the world you can actually read.
const VIGNETTE_CLEAR_INSET_MIN: f32 = 0.08;
/// Fraction of health under which the pulse, the inward crowding, and the
/// full-screen wash ramp in.
const VIGNETTE_CRITICAL: f32 = 0.30;
/// Peak alpha of the full-screen wash at zero health. This is the part that
/// genuinely makes the world *harder to read* rather than merely framing it: the
/// edge gradient alone leaves the middle of the screen perfectly clear, so a
/// player could bleed out at 2 HP and still see everything that matters. The wash
/// only ramps in with `critical`, so a scratched-but-stable player never gets a
/// red film over the whole view.
const VIGNETTE_WASH_ALPHA: f32 = 120.0;
/// The vignette's red: fully saturated, which looks wrong as a number and is
/// right on screen.
///
/// CALIBRATION NOTE. These two constants are tuned against the *rendered* frame,
/// not against nominal alpha, and the difference is large. The egui overlay is
/// composited through the scene's tonemapper, which both desaturates saturated
/// colours and lands the effective alpha around 40% of what is asked for. A
/// tasteful-looking dark red at a sensible alpha (the original `120,16,16` at
/// ~150) therefore arrives on screen as a muddy grey-brown that mostly *dims* a
/// sunlit scene rather than reddening it. Measured on a bright plains frame at
/// 12 HP: the old values shifted the corner pixel's redness by +2; these shift it
/// by ~+25, which is the point at which it actually reads as blood.
///
/// So: if you are eyeballing these numbers and they look garish, check a
/// screenshot before you "fix" them.
const VIGNETTE_RED: (u8, u8, u8) = (255, 32, 24);

/// Red screen-edge vignette that fades in as the local player's health drops
/// below [`VIGNETTE_THRESHOLD`]. Drawn as a single gradient mesh, opaque at the
/// screen border and transparent toward the center.
///
/// Three things ramp together, so the effect reads as *progressively harder to
/// see* rather than a binary "you are hurt" flag:
/// - the edge **alpha** rises on a curve toward [`VIGNETTE_PEAK_ALPHA`],
/// - the clear center **shrinks** from [`VIGNETTE_CLEAR_INSET_MAX`] toward
///   [`VIGNETTE_CLEAR_INSET_MIN`], so the red closes in around the crosshair, and
/// - a faint full-screen **wash** ([`VIGNETTE_WASH_ALPHA`]) fades in once health
///   is critical, so even the middle of the view goes red.
///
/// The center never fully occludes: the wash stays well under half opacity, so a
/// dying player can still aim. Pulses gently once health is critical to nudge them
/// toward retreating or bandaging.
pub(super) fn low_health_vignette(ctx: &egui::Context, health: f32) {
    let Some(shape) = vignette_shape(health) else {
        return;
    };

    // Gentle 1.5 Hz pulse, scaled in by `critical` only, so a stable-but-
    // scratched player gets a calm edge tint and never a throbbing one. Kept
    // out of `vignette_shape` so that stays a pure function of health.
    let time = ctx.input(|input| input.time) as f32;
    let wave = 0.85 + 0.15 * (time * std::f32::consts::TAU * 1.5).sin();
    let pulse = 1.0 + (wave - 1.0) * shape.critical;

    let peak_alpha = (shape.edge_alpha * pulse).clamp(0.0, 255.0) as u8;
    let wash_alpha = (shape.wash_alpha * pulse).clamp(0.0, 255.0) as u8;
    if peak_alpha == 0 && wash_alpha == 0 {
        return;
    }

    // A background-layer painter spans the whole screen, so the gradient mesh
    // isn't clipped to an auto-sized Area (which collapses to nothing since we
    // only paint and never allocate widgets).
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("low_health_vignette"),
    ));
    let (r, g, b) = VIGNETTE_RED;
    let edge = egui::Color32::from_rgba_unmultiplied(r, g, b, peak_alpha);
    // The gradient fades to the WASH alpha at the center rather than to fully
    // transparent, so the two effects are one mesh: the border is the vignette and
    // the interior is the wash, with a smooth ramp between them.
    let center = egui::Color32::from_rgba_unmultiplied(r, g, b, wash_alpha);
    let inner = screen.shrink(screen.size().min_elem() * shape.clear_inset);
    painter.add(vignette_mesh(screen, inner, edge, center));
    // The mesh only covers the border ring (outer rect minus inner rect), so the
    // inner rect needs the wash painted flat.
    if wash_alpha > 0 {
        painter.rect_filled(inner, 0.0, center);
    }

    // Keep the pulse animating while the vignette is visible.
    ctx.request_repaint();
}

/// The vignette's geometry and strength at a given health, before the
/// time-varying pulse is folded in. Pure so the ramp can be tested without an
/// egui context or a clock.
#[derive(Debug, Clone, Copy, PartialEq)]
struct VignetteShape {
    /// Un-pulsed edge alpha in `[0, 255]`.
    edge_alpha: f32,
    /// Un-pulsed full-screen wash alpha in `[0, 255]`. Zero until health is
    /// critical.
    wash_alpha: f32,
    /// Clear-center radius as a fraction of the screen's smaller dimension.
    clear_inset: f32,
    /// How deep into the critical band we are: `0` above it, `1` at empty.
    critical: f32,
}

/// Resolve the vignette for `health`, or `None` when the player is healthy
/// enough that the edges stay clean.
fn vignette_shape(health: f32) -> Option<VignetteShape> {
    let fraction = (health / MAX_HEALTH).clamp(0.0, 1.0);
    if fraction >= VIGNETTE_THRESHOLD {
        return None;
    }
    // 0 at the threshold, 1 at empty, then curved so the healthy end of the
    // band stays subtle and the last sliver of health is oppressive.
    let linear = ((VIGNETTE_THRESHOLD - fraction) / VIGNETTE_THRESHOLD).clamp(0.0, 1.0);
    let severity = linear.powf(VIGNETTE_SEVERITY_CURVE);
    let critical = ((VIGNETTE_CRITICAL - fraction) / VIGNETTE_CRITICAL).clamp(0.0, 1.0);

    Some(VignetteShape {
        edge_alpha: severity * VIGNETTE_PEAK_ALPHA,
        wash_alpha: critical * VIGNETTE_WASH_ALPHA,
        // The clear center closes in as health falls. Both bounds are well
        // under half the smaller dimension, so the inner rect can never invert.
        clear_inset: VIGNETTE_CLEAR_INSET_MAX
            - (VIGNETTE_CLEAR_INSET_MAX - VIGNETTE_CLEAR_INSET_MIN) * critical,
        critical,
    })
}

/// Build the four-quad "frame" mesh between `outer` (filled with `edge`) and
/// `inner` (filled with `center`). Each side fades from the screen border
/// inward; the shared corner vertices keep the corners strongest, which is the
/// classic vignette falloff.
fn vignette_mesh(
    outer: egui::Rect,
    inner: egui::Rect,
    edge: egui::Color32,
    center: egui::Color32,
) -> egui::Shape {
    use egui::epaint::{Vertex, WHITE_UV};
    let mut mesh = egui::Mesh::default();

    // 0..=3 outer corners (edge color), 4..=7 matching inner corners (center
    // color), both in TL, TR, BR, BL order.
    let corners = [
        (outer.left_top(), edge),
        (outer.right_top(), edge),
        (outer.right_bottom(), edge),
        (outer.left_bottom(), edge),
        (inner.left_top(), center),
        (inner.right_top(), center),
        (inner.right_bottom(), center),
        (inner.left_bottom(), center),
    ];
    for (pos, color) in corners {
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color,
        });
    }

    // Four border quads (top, right, bottom, left), each split into two
    // triangles. Indices reference the vertex list above.
    let quads: [(u32, u32, u32, u32); 4] = [
        (0, 1, 5, 4), // top
        (1, 2, 6, 5), // right
        (2, 3, 7, 6), // bottom
        (3, 0, 4, 7), // left
    ];
    for (o0, o1, i1, i0) in quads {
        mesh.indices.extend_from_slice(&[o0, o1, i1, o0, i1, i0]);
    }

    egui::Shape::mesh(mesh)
}

#[cfg(test)]
mod tests {
    use super::super::raw_input;
    use super::*;

    #[test]
    fn low_health_vignette_appears_only_when_hurt() {
        // Full health: the edges stay clean (nothing drawn).
        let ctx = egui::Context::default();
        let healthy = ctx.run_ui(raw_input(), |ui| low_health_vignette(ui.ctx(), MAX_HEALTH));
        assert!(healthy.shapes.is_empty());

        // Critically low: the gradient mesh is emitted.
        let ctx = egui::Context::default();
        let hurt = ctx.run_ui(raw_input(), |ui| {
            low_health_vignette(ui.ctx(), MAX_HEALTH * 0.1);
        });
        assert!(!hurt.shapes.is_empty());
    }

    #[test]
    fn vignette_ramps_in_from_the_threshold_and_crowds_inward_as_health_empties() {
        // Above the threshold: nothing at all.
        assert!(vignette_shape(MAX_HEALTH).is_none());
        assert!(vignette_shape(MAX_HEALTH * VIGNETTE_THRESHOLD).is_none());

        // Just inside the band: present, but barely (the severity curve keeps
        // a scratch from tinting the screen).
        let scratched = vignette_shape(MAX_HEALTH * 0.70).expect("in band");
        assert!(
            scratched.edge_alpha < 8.0,
            "a light wound should be nearly invisible, got {}",
            scratched.edge_alpha
        );
        // Above the critical band the center stays as open as it ever gets, and
        // there is NO full-screen wash: a stable-but-scratched player must never
        // get a red film over the middle of their view.
        assert_eq!(scratched.critical, 0.0);
        assert_eq!(scratched.wash_alpha, 0.0);
        assert!((scratched.clear_inset - VIGNETTE_CLEAR_INSET_MAX).abs() < 1e-6);

        // Alpha rises monotonically and the clear center never grows, all the
        // way down. This is the "harder and harder to see" property.
        let mut previous = vignette_shape(MAX_HEALTH * 0.74).expect("in band");
        for step in 1..=74 {
            let health = MAX_HEALTH * (0.74 - step as f32 * 0.01);
            let shape = vignette_shape(health).expect("in band");
            assert!(
                shape.edge_alpha >= previous.edge_alpha,
                "alpha must not fall as health drops (at {health})"
            );
            assert!(
                shape.clear_inset <= previous.clear_inset + 1e-6,
                "the clear center must not reopen as health drops (at {health})"
            );
            assert!(
                shape.wash_alpha >= previous.wash_alpha,
                "the wash must not fade as health drops (at {health})"
            );
            previous = shape;
        }

        // At empty: peak strength, tightest clear center, fully critical. The
        // center is still a real gap, so the crosshair is never occluded.
        let empty = vignette_shape(0.0).expect("in band");
        assert!((empty.edge_alpha - VIGNETTE_PEAK_ALPHA).abs() < 1e-3);
        assert!((empty.wash_alpha - VIGNETTE_WASH_ALPHA).abs() < 1e-3);
        assert!((empty.clear_inset - VIGNETTE_CLEAR_INSET_MIN).abs() < 1e-6);
        assert_eq!(empty.critical, 1.0);
        assert!(empty.clear_inset > 0.0);
        // Even at zero health the wash stays well under half opacity, so a dying
        // player can still see enough of the world to fight or flee. "Harder to
        // see" is the goal; blind is not.
        assert!(
            empty.wash_alpha < 128.0,
            "the wash must never blind the player"
        );
        // Under half the smaller screen dimension, so `Rect::shrink` can never
        // invert the inner rect.
        assert!(empty.clear_inset < 0.5 && VIGNETTE_CLEAR_INSET_MAX < 0.5);
    }
}
