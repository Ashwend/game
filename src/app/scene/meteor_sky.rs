//! Airborne meteor shower VFX: the burning-rock fireball body, its segmented
//! fiery trail, and the ember/smoke stream it sheds in flight.
//!
//! This is the SKY half of the meteor shower event; the ground half (crater,
//! site fires, rock blast, strike cues) lives in `scene::meteor_shower`. The
//! fireball is a true world-anchored object, placed each frame from the shared
//! deterministic trajectory (`crate::world::meteor_shower`) evaluated against
//! the local clock estimate; see [`MeteorVisual`] and
//! [`update_meteor_sky_system`] for the far-plane proxy scheme that keeps it
//! renderable and correctly sized from kilometres out. The body and trail
//! entities are spawned once by [`setup_meteor_sky`] (called from `setup_scene`
//! right after the sky rig) and repositioned/reshaded per frame; the shed
//! embers are short-lived world-space particles.

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::state::{ClientRuntime, MenuState},
    util::hash::hashed_unit,
    world::chunk::splitmix64,
};

use super::{
    MeteorEmberAssets,
    components::MainCamera,
    sky::{CameraTransformQuery, MoonLight, MoonVisual, SunLight, lerp},
};

/// Which of the three co-moving fireball body layers an entity is. The body is a
/// BURNING ROCK: a dark, irregular, near-black charred-rock CORE, a hot additive
/// orange flame HALO wrapping it (SHELL), and a small white-hot leading CAP over
/// its nose (CORONA, offset forward along travel). That gives the "dark stone with
/// saturated fire raking over it" read instead of the pale cream egg (or the pink
/// halo) the earlier passes produced. One query with a `&MeteorBodyLayer` reaches
/// all three.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeteorBodyLayer {
    /// Opaque, near-black, IRREGULAR charred rock: the solid dark heart, so the
    /// silhouette centre reads dark. Writes depth, occludes the flame behind it.
    Core,
    /// Additive hot-orange flame HALO wrapping the whole rock (larger than it), the
    /// main fire read. Additive so it adds saturated hue over the sky, brightest at
    /// the grazing rim.
    Shell,
    /// White-hot leading CAP: a small very-hot additive blob offset FORWARD along
    /// travel over the rock's nose, the incandescent shock front.
    Corona,
}

/// How a body layer renders. See the `body_layer` builder in `setup_meteor_sky`
/// for the per-mode reasoning (irregular dark rock / additive flame halo /
/// additive cap).
#[derive(Clone, Copy)]
enum MeteorBodyRender {
    /// Opaque, default back-face cull, IRREGULAR mesh: the solid dark rock heart.
    OpaqueRock,
    /// Opaque, FRONT-face cull: the far bowl showing a saturated orange flame ring.
    OpaqueRing,
    /// Additive, back-face cull: the small white-hot leading cap.
    AdditiveCap,
}

/// The meteor shower fireball body, a **true world-anchored object**, not a disc on
/// the sky dome. `update_meteor_sky_system` computes its real world position from
/// the shared committed trajectory each frame. When that position is farther than
/// [`METEOR_PROXY_DISTANCE`] from the camera (which it is for most of the flight,
/// well past the 300 m far plane) the mesh is drawn on a far-plane *proxy*:
/// pulled in along the true direction to the proxy sphere and shrunk by the same
/// ratio, so it keeps its true apparent angular size while staying renderable.
/// Once the object dives inside the proxy sphere it is drawn at its true position
/// and true scale, so players near the impact watch it scream overhead and land
/// in the actual world. Unlike the moon it moves with real parallax: walk toward
/// the impact and it slides across the sky like the physical object it is. Three
/// sibling entities carry it (tagged [`MeteorBodyLayer`]); the trail and embers
/// are further world-positioned siblings. Hidden when no event is in flight or
/// after impact.
#[derive(Component)]
pub(crate) struct MeteorVisual;

/// One segment of the fireball's tapered fiery trail. The trail is a chain of
/// [`METEOR_TRAIL_SEGMENTS`] cones (not one stretched sphere: a comet is mostly
/// tail, and a long segmented streak that narrows to a fine point reads as fire
/// rather than the short fat "leaf/stem" the single teardrop produced). Each is a
/// TOP-LEVEL world entity (not a child of [`MeteorVisual`], which rendered
/// invisible in this Bevy version) positioned directly each frame behind the
/// ball. `index` is 0 at the root (widest, hugging the ball) up to
/// `METEOR_TRAIL_SEGMENTS - 1` at the fine tip.
#[derive(Component)]
pub(crate) struct MeteorTrailSegment {
    index: usize,
}

/// One short-lived particle shed behind the fireball: either a bright ember spark
/// streaking off the tail or a faint dark smoke puff drifting under it, chosen by
/// `smoke`. A world-space entity (lingers where shed, does not follow the meteor)
/// that drifts, shrinks or grows, and despawns. One component so
/// [`tick_meteor_ember_system`] stays a single query pass.
#[derive(Component)]
pub(crate) struct MeteorEmber {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
    /// `true` for a smoke puff (grows over life, drifts, fades alpha), `false`
    /// for a spark (gravity + light drag, shrinks to a point).
    smoke: bool,
}

/// Base radius of the fireball sphere in **world metres**, at the reference
/// distance of the far-plane proxy ([`METEOR_PROXY_DISTANCE`]). This is the size
/// the ball reads when far out on the proxy at the ENTRY of its descent; it grows
/// from there as the meteor nears (see [`meteor_render_placement`]). A physically
/// tiny rock would be a sub-pixel speck kilometres out, so the meteor is drawn as
/// a glowing plasma fireball a good deal larger than the rock: the HDR core +
/// bloom + trail are the burning envelope, and the whole thing swells as it dives.
const METEOR_BASE_RADIUS: f32 = 5.5;

/// Distance from the camera, in metres, the far-plane proxy sits at. Inside the
/// 300 m gameplay far plane with margin so the proxied mesh never clips the
/// frustum edge, and the reference distance the base radius is sized for.
const METEOR_PROXY_DISTANCE: f32 = 250.0;

/// Apparent-size growth from entry to the moment the fireball reaches the proxy
/// boundary. On the proxy the mesh is drawn at `METEOR_BASE_RADIUS * (1 +
/// descent * (this - 1))`, so it starts as a legible burning point at entry and
/// swells to `this`x by the time it dives inside the proxy distance, reading as an
/// object visibly bearing down. Preserving literal angular size instead would pin
/// a distant rock to a static speck (the first world-space pass did exactly that,
/// and it read as a motionless dot); dramatic apparent growth is the point.
const METEOR_PROXY_GROWTH: f32 = 4.0;

/// Once the fireball dives WITHIN this distance of the camera it is drawn at its
/// TRUE world position and TRUE world scale (`METEOR_BASE_RADIUS` in metres), so
/// a player near the impact sees it scream past at physical size with full
/// parallax. Between here and the proxy distance the placement blends from the
/// proxy to the true position so there is no pop. Sized so the true-scale ball
/// (a few metres) matches the apparent size the grown proxy had at the boundary.
const METEOR_TRUE_SCALE_DISTANCE: f32 = 200.0;

// Meteor colour/brightness. The fireball is a BURNING ROCK, not a glowing ball.
// Fire runs HOT-to-COOL: white/yellow at the hottest point, then orange, then deep
// red, NEVER pink. The prior pass built the flame from an additive DEEP-RED wisp
// over the blue-ish sky, which mixes to PINK/SALMON (red + blue = magenta), so the
// halo read as bubblegum, not fire. It is rebuilt so the flame runs hot-to-cool and
// the rock heart stays DARK:
//
// The three co-moving body meshes are repurposed (all still `unlit`, which in this
// Bevy version outputs `base_color` directly into the HDR buffer WITHOUT `emissive`
// and WITHOUT exposure, so the base colour IS the emitter):
//   - CORE is an OPAQUE, genuinely DARK, near-black charred rock, built from an
//     IRREGULAR (vertex-perturbed) low-poly sphere so its silhouette is a jagged
//     stone, not a smooth ball. Deliberately near-black so the centre reads as a
//     dark mass with fire raking over it. It writes depth, so the flame BEHIND it
//     is hidden and the flame IN FRONT of it (the leading cap) adds over it.
//   - SHELL is the ADDITIVE saturated FLAME ENVELOPE, a hot ORANGE-YELLOW halo
//     wrapping the whole rock (larger than it, so it is a real halo of fire, not a
//     thin rim). Enough green to stay orange (never pink); bright enough to bloom
//     but held under the level that clips the disc to flat white.
//   - CORONA is repurposed as the WHITE-HOT LEADING CAP: a small, very hot
//     white/yellow additive blob offset FORWARD along travel, so the leading face
//     of the rock reads incandescent (the ram-pressure shock front) while the rest
//     of the rock stays dark. This is the "hot leading edge" the review demanded.
// Every layer's `base_color` lerps ENTRY -> IMPACT with the descent (hotter the
// closer it gets) and shimmers with the seeded flicker.

/// CORE (the charred rock heart) radius as a fraction of [`METEOR_BASE_RADIUS`].
/// The dark solid stone; the flame halo hugs it and the leading cap sits just off
/// its nose.
const METEOR_CORE_RADIUS_FRAC: f32 = 0.55;
/// SHELL (flame envelope) radius fraction. Only a touch larger than the rock so the
/// OPAQUE front-culled flame bowl shows a saturated-orange RING in a THIN annulus
/// just past the rock silhouette, not a wide disc that swallows the stone.
const METEOR_SHELL_RADIUS_FRAC: f32 = 0.63;
/// CORONA, now the WHITE-HOT LEADING CAP radius fraction: a SMALL hot blob offset
/// forward along travel over the leading face of the rock. Kept small so the hottest
/// (nearly white) spot is a compact incandescent point, not a wide wash.
const METEOR_CORONA_RADIUS_FRAC: f32 = 0.34;
/// How far forward (along travel, in ball radii) the white-hot leading cap sits, so
/// it caps the NOSE of the rock rather than its centre.
const METEOR_LEADING_CAP_OFFSET_FRAC: f32 = 0.30;

/// OPAQUE charred-rock core linear base_color at entry / impact. NEAR-BLACK charred
/// stone with the faintest warm ember bias, so the silhouette centre reads as a
/// dark burning rock. Deliberately tiny values (well under 1.0) so it never blooms:
/// the darkness is the whole point, fire is what glows, not the stone.
const METEOR_CORE_ENTRY: Vec3 = Vec3::new(0.020, 0.010, 0.006);
const METEOR_CORE_IMPACT: Vec3 = Vec3::new(0.11, 0.035, 0.010);
/// OPAQUE flame-envelope shell linear base_color at entry / impact. Rendered as a
/// solid FRONT-culled bowl so the far interior wall shows DIRECTLY (opaque, so its
/// colour survives AgX + bloom instead of washing to cream like an additive glow
/// does) as a saturated-orange RING in the annulus past the rock. A hot saturated
/// orange (green ~0.32 of red so it is orange, not yellow; blue near zero so it
/// never drifts pink). Held moderate so it blooms without clipping to a flat white.
// AgX (this game's tonemapper) desaturates bright warm colours hard and lifts blue,
// washing a bright orange disc to tan. Fire therefore has to be DEEP and SATURATED,
// sitting in AgX's lower-mid range where hue survives, with only tiny hot cores
// (the leading cap, ember sparks) allowed to go bright. So this is a deep blood-
// orange, not a bright one: moderate red, near-zero green, zero blue.
// AgX (this game's tonemapper) washes ANY large bright warm area to cream, no matter
// the input ratio; a smooth bright orange disc always tonemaps to tan. Saturated
// orange only survives as a DEEP orange (moderate red, near-zero green/blue) that
// sits just above the sky, so the flame envelope is kept deep + thin and the bright
// SATURATED read is carried by the small high-contrast ember sparks (which hold hue
// because they are tiny points on dark sky) and a compact white-hot leading cap.
const METEOR_SHELL_ENTRY: Vec3 = Vec3::new(1.35, 0.15, 0.0);
const METEOR_SHELL_IMPACT: Vec3 = Vec3::new(1.9, 0.22, 0.0);
/// ADDITIVE white-hot leading cap linear base_color at entry / impact. Hotter than
/// the halo (high red + strong green so it reads white-yellow, the hottest part of
/// the fire, with a little blue so the very core tips toward white without going
/// cyan) but kept MODERATE so it stays a saturated incandescent nose, not a flat
/// white disc that swallows the rock. Sits over the rock's nose only.
const METEOR_CORONA_ENTRY: Vec3 = Vec3::new(1.5, 0.52, 0.05);
const METEOR_CORONA_IMPACT: Vec3 = Vec3::new(2.2, 0.80, 0.10);

/// Forward-leaning ovoid stretch of the body along travel (local +Z after the
/// NEG_Z -> travel mapping) at entry / impact. Slightly stronger near impact so
/// the ball leans into its plunge.
const METEOR_BODY_STRETCH_ENTRY: f32 = 1.15;
const METEOR_BODY_STRETCH_IMPACT: f32 = 1.30;

// The trail is a chain of [`METEOR_TRAIL_SEGMENTS`] additive cones that narrows
// to a fine point, root->tip, so it reads as a long fiery comet streak rather
// than the short fat teardrop (the "leaf") the single stretched sphere produced.

/// Number of cone segments in the tapered trail chain.
const METEOR_TRAIL_SEGMENTS: usize = 6;
/// Total tail length as a multiple of the rendered BALL RADIUS at entry / impact.
/// A comet is mostly tail, so this is long: many body-lengths, tapering to a
/// point. (Sized in world units off the ball radius so it reads at the ball's
/// apparent size from any distance.)
const METEOR_TRAIL_LENGTH_ENTRY: f32 = 8.0;
const METEOR_TRAIL_LENGTH_IMPACT: f32 = 20.0;
/// Fraction of the total tail length each segment spans, root -> tip. Unequal so
/// the near-ball part is dense and the far part is a long whisker. Sums to 1.0.
const METEOR_TRAIL_SEG_LEN_FRAC: [f32; METEOR_TRAIL_SEGMENTS] =
    [0.10, 0.13, 0.16, 0.19, 0.20, 0.22];
/// Root base half-width of the trail as a fraction of the ball radius; each
/// segment tapers by `(1 - k/N)^1.3` off this. WIDE at the root (nearly the ball
/// width) so the flame is a fat violent band dragging off the stone, tapering to a
/// fine point over a long tail (a comet is mostly tail).
const METEOR_TRAIL_ROOT_WIDTH_FRAC: f32 = 0.72;
/// Trail intensity (multiplier on the per-segment hue) at the ROOT, entry /
/// impact. The root hue is already white-hot; a modest multiplier keeps it from
/// clipping the whole root to a flat white slab while still blooming.
const METEOR_TRAIL_ROOT_INTENSITY_ENTRY: f32 = 0.85;
const METEOR_TRAIL_ROOT_INTENSITY_IMPACT: f32 = 1.0;
/// Trail intensity at the fine TIP, entry / impact. Held up enough that the tail
/// stays a visible deep-red flame the whole way out instead of vanishing to a thin
/// pale whisker; the tip HUE (deep red) carries the cooling read, not darkness.
const METEOR_TRAIL_TIP_INTENSITY_ENTRY: f32 = 0.55;
const METEOR_TRAIL_TIP_INTENSITY_IMPACT: f32 = 0.9;
/// Per-segment hue gradient (linear rgb), running HOT-to-COOL root -> mid -> tip:
/// WHITE-YELLOW at the root (the hottest flame, high R+G with a little B so it tips
/// toward white, never pink), through saturated orange at the mid, to a deep
/// ember-red at the tip. HDR values (well over 1.0 at the root) so the opaque cones
/// stay incandescent, not tan, under the day tonemapper.
const METEOR_TRAIL_HUE_ROOT: Vec3 = Vec3::new(0.95, 0.13, 0.0);
const METEOR_TRAIL_HUE_MID: Vec3 = Vec3::new(0.62, 0.045, 0.0);
const METEOR_TRAIL_HUE_TIP: Vec3 = Vec3::new(0.32, 0.010, 0.0);
/// Peak lateral waver amplitude as a fraction of the ball radius, at the tip. The
/// waver is a bounded lateral lash around the analytic spine (root stiff, tip
/// loose), a flame flicker, NOT a re-aim (it must not reintroduce the "pointing
/// around" the world-space rework fixed).
const METEOR_TRAIL_WAVER_FRAC: f32 = 0.22;

/// Ember spark rate (per second) far out (proxy) and close (true scale). Ramps up
/// as the fireball nears so the dense streaking stream only shows in the last
/// seconds; far out it is a steady sputter (bumped up so the shed embers are
/// clearly visible even mid-flight, not just in the final seconds).
const METEOR_EMBER_RATE_FAR: f32 = 90.0;
const METEOR_EMBER_RATE_CLOSE: f32 = 240.0;
/// Downward acceleration (m/s^2) applied to shed sparks so they arc off the tail.
const METEOR_EMBER_GRAVITY: f32 = 9.0;

/// Spawn the meteor shower fireball and its trail chain, all `Hidden` until an
/// event is in flight. Called from `setup_scene` right after `setup_sky` so the
/// meteor rig spawns alongside the rest of the sky.
///
/// The fireball is three co-moving unlit fog-immune meshes placed at a TRUE
/// world position (or its far-plane proxy) by `update_meteor_sky_system`, with
/// HDR heat in each `base_color` (the unlit path skips both `emissive` and
/// exposure, so the base colour IS the HDR emitter). It reads as a BURNING ROCK:
/// an OPAQUE, IRREGULAR, near-black charred-rock CORE (the dark heart), an
/// ADDITIVE hot-orange flame HALO wrapping it (SHELL), and an ADDITIVE white-hot
/// leading CAP (CORONA) offset forward over the nose. Additive on the two fire
/// layers so they add saturated hue, never washing to cream or drifting pink; the
/// opaque rock writes depth so the flame behind it is hidden and the front flame
/// adds over it. The trail is a separate chain of cones (spawned below); both
/// trail and embers are further world-positioned siblings (parenting made them
/// invisible in this Bevy version). Hidden until an event is in flight; brightness
/// rewritten per frame.
pub(super) fn setup_meteor_sky(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let body_layer = |commands: &mut Commands,
                      meshes: &mut Assets<Mesh>,
                      materials: &mut Assets<StandardMaterial>,
                      layer: MeteorBodyLayer,
                      name: &str,
                      radius_frac: f32,
                      entry: Vec3,
                      render: MeteorBodyRender| {
        // The rock CORE gets an irregular, vertex-perturbed mesh so its silhouette
        // is a jagged stone, not a smooth ball; the two fire layers stay smooth
        // spheres (the additive glow does not want facets).
        let radius = METEOR_BASE_RADIUS * radius_frac;
        let mesh = match render {
            MeteorBodyRender::OpaqueRock => meshes.add(irregular_rock_mesh(radius, 0x1234_5678)),
            _ => meshes.add(
                Sphere::new(radius)
                    .mesh()
                    .ico(3)
                    .expect("valid subdivisions"),
            ),
        };
        // Three render modes for the three layers:
        //  - `OpaqueRock`: the dark irregular stone. Default back-face cull, writes
        //    depth, so the silhouette centre reads dark and the flame behind it is
        //    occluded.
        //  - `OpaqueRing`: the flame envelope. OPAQUE, FRONT-face cull: only the FAR
        //    interior wall of a sphere just larger than the rock draws. At the
        //    silhouette centre that wall sits behind the opaque rock and is depth-
        //    hidden, so the rock stays dark; past the rock's rim it shows as a solid
        //    saturated-orange RING. Opaque (not additive) so its hue survives AgX +
        //    bloom instead of washing to cream, the same lesson the opaque trail
        //    proved. That is the "dark stone with fire raking the rim" read.
        //  - `AdditiveCap`: the white-hot leading cap. Additive, back-face cull (near
        //    hemisphere), a small hot blob the update loop pushes forward along travel
        //    so it caps the nose.
        let (alpha_mode, cull_mode) = match render {
            MeteorBodyRender::OpaqueRock => {
                (AlphaMode::Opaque, StandardMaterial::default().cull_mode)
            }
            MeteorBodyRender::OpaqueRing => (
                AlphaMode::Opaque,
                Some(bevy::render::render_resource::Face::Front),
            ),
            MeteorBodyRender::AdditiveCap => {
                (AlphaMode::Add, StandardMaterial::default().cull_mode)
            }
        };
        let material = materials.add(StandardMaterial {
            base_color: Color::linear_rgb(entry.x, entry.y, entry.z),
            unlit: true,
            fog_enabled: false,
            alpha_mode,
            cull_mode,
            ..default()
        });
        commands.spawn((
            Name::new(name.to_string()),
            MeteorVisual,
            layer,
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            NotShadowCaster,
        ));
    };
    body_layer(
        commands,
        meshes,
        materials,
        MeteorBodyLayer::Core,
        "Meteor Core",
        METEOR_CORE_RADIUS_FRAC,
        METEOR_CORE_ENTRY,
        MeteorBodyRender::OpaqueRock,
    );
    body_layer(
        commands,
        meshes,
        materials,
        MeteorBodyLayer::Shell,
        "Meteor Shell",
        METEOR_SHELL_RADIUS_FRAC,
        METEOR_SHELL_ENTRY,
        MeteorBodyRender::OpaqueRing,
    );
    body_layer(
        commands,
        meshes,
        materials,
        MeteorBodyLayer::Corona,
        "Meteor Corona",
        METEOR_CORONA_RADIUS_FRAC,
        METEOR_CORONA_ENTRY,
        MeteorBodyRender::AdditiveCap,
    );

    // Trail: a chain of OPAQUE truncated cones (frustums) narrowing to a fine
    // point. Opaque, not additive: additive orange washed to cream over the bright
    // day sky (screenshot-confirmed), so the streak occludes the sky like the crater
    // ember glow to keep its saturated orange in daylight. Each frustum runs base_w
    // -> apex_w along its local +Y, and its apex width equals the next segment's base
    // width, so the chain is C0-continuous and reads as one smooth taper rather than
    // six stacked blobs. Unit-height meshes (the +Y span is 1.0) that the per-frame
    // update scales to the segment length; the widths are baked into the mesh.
    // Oriented and positioned per frame so the chain drags straight behind travel.
    for index in 0..METEOR_TRAIL_SEGMENTS {
        let base_w = trail_segment_root_width(index);
        let apex_w = trail_segment_root_width(index + 1);
        let mesh = meshes.add(trail_frustum_mesh(base_w, apex_w));
        let material = materials.add(StandardMaterial {
            base_color: Color::linear_rgb(1.0, 0.4, 0.1),
            unlit: true,
            fog_enabled: false,
            cull_mode: None,
            ..default()
        });
        commands.spawn((
            Name::new(format!("Meteor Trail Segment {index}")),
            MeteorTrailSegment { index },
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }
}

/// Build an irregular, low-poly "charred rock" mesh: an ico-sphere of the given
/// radius whose vertices are pushed in/out along their normal by a fixed hashed
/// jitter, so the silhouette is a jagged stone rather than a smooth ball. Flat
/// normals are recomputed afterward so each facet catches the fire's additive glow
/// as a distinct plane (it reads faceted, like the cel ore boulders). Deterministic
/// (seeded from the vertex index) so the rock shape is stable across frames.
/// A lumpy faceted rock: an icosphere with seeded per-vertex radial jitter and
/// flat normals. Shared by the meteor's stone core and the meteor shower blast
/// debris (`scene::meteor_shower`), which builds several variants off different
/// seeds so the flung chunks aren't clones.
pub(crate) fn irregular_rock_mesh(radius: f32, seed: u32) -> Mesh {
    use bevy::mesh::VertexAttributeValues;

    let mut mesh = Sphere::new(radius)
        .mesh()
        .ico(2)
        .expect("valid subdivisions");
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for (i, p) in positions.iter_mut().enumerate() {
            let v = Vec3::from_array(*p);
            let len = v.length();
            if len <= f32::EPSILON {
                continue;
            }
            // Two hashed draws per vertex for a lumpier displacement; +/- ~28% of
            // the radius so the rock is visibly irregular but never self-intersects.
            let h1 = hashed_unit((i as u32).wrapping_mul(0x9E37_79B9) ^ seed);
            let h2 = hashed_unit((i as u32).wrapping_mul(0x85EB_CA6B) ^ seed.rotate_left(16));
            let jitter = 1.0 + (h1 - 0.5) * 0.42 + (h2 - 0.5) * 0.14;
            let scaled = v / len * (len * jitter);
            *p = scaled.to_array();
        }
    }
    // Flat per-face normals so the facets read as distinct planes under the glow.
    mesh.duplicate_vertices();
    mesh.compute_normals();
    mesh
}

/// Root half-width (as a fraction of the ball radius) of trail segment `k`, the
/// taper curve `ROOT_WIDTH * (1 - k/N)^1.5`. At `k == METEOR_TRAIL_SEGMENTS` it is
/// 0.0, a true point for the last segment's apex. Pure so the spawn and update
/// agree on the shared taper.
fn trail_segment_root_width(k: usize) -> f32 {
    if k >= METEOR_TRAIL_SEGMENTS {
        return 0.0;
    }
    let frac = 1.0 - (k as f32) / (METEOR_TRAIL_SEGMENTS as f32);
    METEOR_TRAIL_ROOT_WIDTH_FRAC * frac.powf(1.3)
}

/// A unit-height open truncated cone (frustum) tapering from `base_r` (radius at
/// local `y = 0`) to `apex_r` (radius at local `y = 1`), around the +Y axis.
/// Bevy's `Cone` primitive tapers to a single point, so a frustum with a non-zero
/// apex (needed for a C0-continuous chain) is built by hand here: a
/// `resolution`-sided open band, double-sided (via the material's `cull_mode:
/// None`), no caps. Radii are in ball-radius fractions; the per-frame transform
/// scales the whole thing to world units.
fn trail_frustum_mesh(base_r: f32, apex_r: f32) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    const RES: usize = 12;
    let base_r = base_r.max(1e-4);
    let apex_r = apex_r.max(0.0);

    let mut positions: Vec<[f32; 3]> = Vec::with_capacity((RES + 1) * 2);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity((RES + 1) * 2);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity((RES + 1) * 2);
    for i in 0..=RES {
        let theta = (i as f32) / (RES as f32) * std::f32::consts::TAU;
        let (s, c) = theta.sin_cos();
        // Base ring (y = 0) then apex ring (y = 1).
        positions.push([base_r * c, 0.0, base_r * s]);
        positions.push([apex_r * c, 1.0, apex_r * s]);
        // Radial-ish normal (good enough for an unlit additive emitter).
        normals.push([c, 0.0, s]);
        normals.push([c, 0.0, s]);
        let u = (i as f32) / (RES as f32);
        uvs.push([u, 0.0]);
        uvs.push([u, 1.0]);
    }
    let mut indices: Vec<u32> = Vec::with_capacity(RES * 6);
    for i in 0..RES {
        let b0 = (i * 2) as u32;
        let a0 = b0 + 1;
        let b1 = b0 + 2;
        let a1 = b0 + 3;
        // Two triangles per side quad (b0, b1, a1) and (b0, a1, a0).
        indices.extend_from_slice(&[b0, b1, a1, b0, a1, a0]);
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

type MeteorVisualQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Visibility,
        &'static MeteorBodyLayer,
        &'static MeshMaterial3d<StandardMaterial>,
    ),
    (
        With<MeteorVisual>,
        Without<MoonVisual>,
        Without<SunLight>,
        Without<MoonLight>,
        Without<MainCamera>,
    ),
>;

type MeteorTrailQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Visibility,
        &'static MeteorTrailSegment,
        &'static MeshMaterial3d<StandardMaterial>,
    ),
    (
        Without<MeteorVisual>,
        Without<MoonVisual>,
        Without<SunLight>,
        Without<MoonLight>,
        Without<MainCamera>,
    ),
>;

/// Resolve where to draw the fireball, at what scale, and how "close" it is.
/// Returns the render translation, the render-scale multiplier on top of
/// [`METEOR_BASE_RADIUS`], and a `true_scale_factor` in `[0, 1]` (1 within
/// [`METEOR_TRUE_SCALE_DISTANCE`], 0 beyond [`METEOR_PROXY_DISTANCE`], linear
/// between) used to LOD-gate the dense close-pass ember stream. Pure so the
/// placement is unit-testable without a camera.
///
/// Three regimes, blended so there is no pop:
/// - **Far (beyond [`METEOR_PROXY_DISTANCE`]):** drawn on the far-plane proxy
///   sphere along the true bearing (so parallax and direction are exact and it
///   stays inside the 300 m frustum), at a scale that GROWS with `descent` from
///   1x at entry to [`METEOR_PROXY_GROWTH`]x at the boundary. This is the visible
///   drama: the object reads as bearing down and swelling, not a static speck
///   (literal angular size would pin a distant rock to a sub-pixel dot).
/// - **Close (within [`METEOR_TRUE_SCALE_DISTANCE`]):** drawn at its TRUE world
///   position and TRUE world scale (1x), so a player near the impact watches it
///   scream past at physical size with full parallax.
/// - **Between:** the render position lerps from the proxy point to the true
///   position, and the scale from the grown proxy size to 1x, across the
///   `[TRUE_SCALE_DISTANCE, PROXY_DISTANCE]` band.
fn meteor_render_placement(true_pos: Vec3, camera_pos: Vec3, descent: f32) -> (Vec3, f32, f32) {
    let to_meteor = true_pos - camera_pos;
    let distance = to_meteor.length();
    if distance <= f32::EPSILON {
        return (true_pos, 1.0, 1.0);
    }
    let dir = to_meteor / distance;

    // Apparent size on the proxy: grows with descent so the ball bears down.
    let proxy_scale = 1.0 + descent.clamp(0.0, 1.0) * (METEOR_PROXY_GROWTH - 1.0);
    let proxy_pos = camera_pos + dir * METEOR_PROXY_DISTANCE;

    // Close-ness LOD factor: 1 at/inside the true-scale distance, 0 at/beyond the
    // proxy distance, linear in between. Independent of the position/scale blend so
    // callers can gate the dense ember stream on genuine proximity.
    let span = (METEOR_PROXY_DISTANCE - METEOR_TRUE_SCALE_DISTANCE).max(f32::EPSILON);
    let tf = ((METEOR_PROXY_DISTANCE - distance) / span).clamp(0.0, 1.0);

    if distance >= METEOR_PROXY_DISTANCE {
        // Far: pure proxy.
        return (proxy_pos, proxy_scale, tf);
    }
    if distance <= METEOR_TRUE_SCALE_DISTANCE {
        // Close: true position, true (physical) scale.
        return (true_pos, 1.0, tf);
    }
    // Blend band: lerp position proxy -> true and scale proxy_scale -> 1 as the
    // object dives from the proxy distance in to the true-scale distance.
    // t = 0 at the proxy boundary, 1 at the true-scale boundary (same curve as tf).
    let t = tf;
    let render_pos = proxy_pos.lerp(true_pos, t);
    let scale = proxy_scale + (1.0 - proxy_scale) * t;
    (render_pos, scale, tf)
}

/// Ember-emission bookkeeping for the fireball: fractional accumulators (spark +
/// smoke) so the per-second rates produce whole particles across frames, and the
/// impact tick these flags belong to so a new event resets it.
#[derive(Default)]
pub(crate) struct MeteorEmberEmitter {
    event_tick: u64,
    spark_accumulator: f32,
    smoke_accumulator: f32,
    /// Free-running counter salting each particle's random draw so successive
    /// sputters differ.
    spawn_seq: u32,
}

/// Hide every trail segment (used from the fireball's early-return /
/// not-in-flight paths so a stale streak never lingers on the sky).
fn set_trail_hidden(trail: &mut MeteorTrailQuery) {
    for (_, mut visibility, _, _) in trail.iter_mut() {
        *visibility = Visibility::Hidden;
    }
}

/// Hide every fireball body layer (core/shell/corona) on the not-in-flight paths.
fn set_body_hidden(body: &mut MeteorVisualQuery) {
    for (_, mut visibility, _, _) in body.iter_mut() {
        *visibility = Visibility::Hidden;
    }
}

/// Position, orient, size, and shade the meteor shower fireball each frame from the
/// shared deterministic **world-space** trajectory
/// (`crate::world::meteor_shower::meteor_world_state`) evaluated against the local
/// clock estimate, and shed the ember sputter trail behind it.
///
/// The object is a true world entity: the far-plane proxy
/// ([`meteor_render_placement`]) keeps it renderable and correctly sized from any
/// distance while preserving parallax, so players can follow it from a distant
/// burning point all the way to a scream-overhead landing. The trail child's
/// local -Z is aligned with the (analytic, stable) velocity so the streak drags
/// straight behind travel, and both materials' HDR brightness is rewritten per
/// frame (descent ramp x seeded flicker) so the ball visibly burns against day
/// and night skies alike. Runs in `ClientSystemSet::Sky` alongside
/// `update_sky_system`; gated on `!uses_menu_backdrop` (the title screen has no
/// world) per gotcha 12.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn update_meteor_sky_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    time: Res<Time>,
    ember_assets: Option<Res<MeteorEmberAssets>>,
    mut emitter: Local<MeteorEmberEmitter>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: CameraTransformQuery,
    mut meteor: MeteorVisualQuery,
    mut trail: MeteorTrailQuery,
) {
    // Resolve the live event, or hide the fireball + trail. Hidden when: no event,
    // the title backdrop is up, the meteor is not yet in flight, or it has struck.
    let event = if menu.screen.uses_menu_backdrop() {
        None
    } else {
        runtime.meteor_shower
    };
    let Some(event) = event else {
        set_body_hidden(&mut meteor);
        set_trail_hidden(&mut trail);
        return;
    };
    // The FRACTIONAL clock estimate: evaluating the committed arc at whole 20 Hz
    // ticks quantises the plunge into 50 ms position steps, which reads as a
    // stuttering final descent on any client rendering faster than the tick rate.
    let now = runtime.server_tick_precise();
    let Some(state) = crate::world::meteor_world_state(
        Vec2::new(event.impact_position.x, event.impact_position.z),
        event.impact_tick,
        event.trajectory_seed,
        now,
    ) else {
        set_body_hidden(&mut meteor);
        set_trail_hidden(&mut trail);
        return;
    };

    let Ok(camera_transform) = camera.single() else {
        set_body_hidden(&mut meteor);
        set_trail_hidden(&mut trail);
        return;
    };
    let camera_pos = camera_transform.translation;
    let descent = state.descent_fraction;
    let flicker = state.flicker;

    // Place on the far-plane proxy (growing with descent) or, once close, at the
    // true world position and true scale. Preserves parallax and lets the object
    // be followed from a distant burning point to a scream-overhead landing. `tf`
    // is the close-ness LOD factor that gates the dense ember stream.
    let (render_pos, render_scale, tf) =
        meteor_render_placement(state.position, camera_pos, descent);
    let ball_radius = METEOR_BASE_RADIUS * render_scale;

    // Travel direction: the analytic velocity, stable and continuous (no
    // finite-difference jitter). Aligning local -Z with it points the ball's nose
    // along travel; the trail drags straight behind (opposite travel).
    let travel = state.velocity.normalize_or_zero();
    let rotation = if travel != Vec3::ZERO {
        Quat::from_rotation_arc(Vec3::NEG_Z, travel)
    } else {
        Quat::IDENTITY
    };
    // Forward-leaning ovoid: stretch the body's local +Z (the tail axis, since
    // NEG_Z maps to travel) so it leans along the plunge.
    let stretch = lerp(
        METEOR_BODY_STRETCH_ENTRY,
        METEOR_BODY_STRETCH_IMPACT,
        descent,
    );
    let s = render_scale * flicker;
    let body_scale = Vec3::new(s, s, s * stretch);

    // Body: place, orient, size, and shade each of the three co-moving layers. The
    // OPAQUE dark rock CORE and the additive flame HALO (SHELL) are concentric at the
    // render position; the white-hot leading CAP (CORONA) is pushed FORWARD along
    // travel so it caps the rock's nose. Depth just works: the opaque rock writes
    // depth, the front flame adds over it, the rear flame is hidden. The rock also
    // gets a slow deterministic tumble so its facets turn as it falls (a rigid moon
    // would not spin).
    let cap_offset = travel * ball_radius * METEOR_LEADING_CAP_OFFSET_FRAC;
    // Slow tumble for the stone, seeded off the trajectory so it is stable per event.
    let tumble_t = (now / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;
    let tumble = Quat::from_euler(
        EulerRot::XYZ,
        tumble_t * 0.7,
        tumble_t * 0.5,
        tumble_t * 0.3,
    );
    for (mut transform, mut visibility, layer, material) in meteor.iter_mut() {
        *visibility = Visibility::Visible;
        transform.rotation = rotation;
        transform.scale = body_scale;
        transform.translation = render_pos;
        let (entry, impact) = match layer {
            MeteorBodyLayer::Core => {
                // The stone tumbles (about its own centre) inside the fixed halo.
                transform.rotation = rotation * tumble;
                (METEOR_CORE_ENTRY, METEOR_CORE_IMPACT)
            }
            MeteorBodyLayer::Shell => (METEOR_SHELL_ENTRY, METEOR_SHELL_IMPACT),
            MeteorBodyLayer::Corona => {
                // Leading cap: shove it forward over the nose so the incandescent
                // shock front sits on the travel-forward face, not the centre.
                transform.translation = render_pos + cap_offset;
                (METEOR_CORONA_ENTRY, METEOR_CORONA_IMPACT)
            }
        };
        if let Some(mut material) = materials.get_mut(&material.0) {
            let color = entry.lerp(impact, descent.clamp(0.0, 1.0)) * flicker;
            material.base_color = Color::linear_rgb(color.x, color.y, color.z);
        }
    }

    // Trail: a long tapering fiery streak dragged straight behind the ball, built
    // from a chain of frustum segments (world entities, not children). Foreshorten-
    // ing flare + a bounded lateral waver make it read as a comet from any pose.
    update_meteor_trail(
        &mut trail,
        &mut materials,
        render_pos,
        travel,
        camera_transform.forward().as_vec3(),
        ball_radius,
        descent,
        flicker,
        event.trajectory_seed,
        now,
        camera_pos,
    );

    // Ember + smoke stream: shed at the fireball's RENDER position (so they read as
    // coming off the visible ball), each a world-space particle so it lingers and
    // drifts where shed. The dedicated bright ember material (not the dim torch
    // flame) holds orange in daylight; the stream is LOD-gated to the close pass so
    // dozens of additive sparks only appear in the last seconds.
    if let Some(ember_assets) = ember_assets.as_ref() {
        // Foreshortening factor: 0 side-on, 1 head-on (tail pointing at camera).
        let f = foreshortening_factor(travel, camera_transform.forward().as_vec3());
        emit_meteor_embers(
            &mut commands,
            ember_assets,
            &mut emitter,
            event.impact_tick,
            render_pos,
            travel,
            ball_radius,
            descent,
            tf,
            f,
            time.delta_secs(),
        );
    }
}

/// Head-on foreshortening factor in `[0, 1]`: 0 when the tail is broadside to the
/// camera, 1 when travel points at (or away from) the camera so the tail is
/// end-on. Used to flare the trail root and widen the ember spread so an end-on
/// tail still reads as a fiery skirt hugging the ball, not a stub.
fn foreshortening_factor(travel: Vec3, camera_forward: Vec3) -> f32 {
    if travel == Vec3::ZERO {
        return 0.0;
    }
    let back = -travel;
    ((back.dot(camera_forward).abs() - 0.6) / 0.4).clamp(0.0, 1.0)
}

/// Position, orient, size, and shade every trail segment for one frame. The chain
/// walks straight back from the ball along `-travel`, each segment a frustum
/// scaled to its share of the total tail length, with a bounded lateral waver and
/// a root-flare for the head-on pose. Split out to keep the update system legible.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn update_meteor_trail(
    trail: &mut MeteorTrailQuery,
    materials: &mut Assets<StandardMaterial>,
    render_pos: Vec3,
    travel: Vec3,
    camera_forward: Vec3,
    ball_radius: f32,
    descent: f32,
    flicker: f32,
    trajectory_seed: u64,
    now: f64,
    camera_pos: Vec3,
) {
    if travel == Vec3::ZERO || ball_radius <= 0.0 {
        set_trail_hidden(trail);
        return;
    }
    let back = -travel;

    // Total tail length, in world units, off the ball radius. The tail runs along
    // `back` from the ball; clamp `L` so the true tip `render_pos + back * L` stays
    // within the far plane (fog is off, so a hard clip would show). Solve the
    // quadratic `|d + back*L|^2 = R^2` for the positive root, where `d = render_pos
    // - camera_pos`. The tail usually points away from the camera, so this rarely
    // bites; on the proxy (ball ~250 m out) it caps the far end cleanly.
    let mut length = ball_radius
        * lerp(
            METEOR_TRAIL_LENGTH_ENTRY,
            METEOR_TRAIL_LENGTH_IMPACT,
            descent,
        );
    let d = render_pos - camera_pos;
    const FAR_LIMIT: f32 = 290.0;
    // |d + back*L|^2 = |d|^2 + 2 L (d·back) + L^2 = FAR_LIMIT^2 (|back| = 1).
    let b = d.dot(back);
    let c = d.length_squared() - FAR_LIMIT * FAR_LIMIT;
    if c < 0.0 {
        // Ball inside the far limit (the normal case). Largest L with the tip on
        // the sphere is `-b + sqrt(b^2 - c)`; only clamp if the tail would exceed it.
        let max_l = -b + (b * b - c).max(0.0).sqrt();
        length = length.min(max_l.max(0.0));
    }
    if length <= 0.0 {
        set_trail_hidden(trail);
        return;
    }

    // Waver: a bounded lateral lash around the analytic spine (root stiff, tip
    // loose). NOT a re-aim, so it never reintroduces the "pointing around" read.
    let (perp_a, perp_b) = orthonormal_basis(back);
    const WAVER_SALT: u64 = 0xF11C_4E12_0000_0000;
    let phase = (splitmix64(trajectory_seed ^ WAVER_SALT) % 6_283) as f32 / 1_000.0;
    let t = (now / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;

    // Foreshortening flare: widen the first two segments when the tail is end-on so
    // it shows a bright flared skirt hugging the ball instead of a nub.
    let f = foreshortening_factor(travel, camera_forward);
    let flare = lerp(1.0, 1.6, f);

    let root_intensity = lerp(
        METEOR_TRAIL_ROOT_INTENSITY_ENTRY,
        METEOR_TRAIL_ROOT_INTENSITY_IMPACT,
        descent,
    );
    let tip_intensity = lerp(
        METEOR_TRAIL_TIP_INTENSITY_ENTRY,
        METEOR_TRAIL_TIP_INTENSITY_IMPACT,
        descent,
    );

    // Precompute each segment's start point walking back from just behind the ball's
    // back edge (a small gap so the wide bright root does not overlap and wash out the
    // dark rock / thin flame rim into a cream lobe).
    let n = METEOR_TRAIL_SEGMENTS;
    let mut p = render_pos + back * ball_radius * 0.95;
    let mut starts = [Vec3::ZERO; METEOR_TRAIL_SEGMENTS];
    let mut seg_lengths = [0.0f32; METEOR_TRAIL_SEGMENTS];
    for k in 0..n {
        starts[k] = p;
        seg_lengths[k] = length * METEOR_TRAIL_SEG_LEN_FRAC[k];
        p += back * seg_lengths[k];
    }

    // Waver offset per node (index 0..=n): amplitude ~0 at root, up to
    // WAVER_FRAC * ball_radius at the tip.
    let waver_at = |k: usize| -> Vec3 {
        let kf = k as f32;
        let amp = ball_radius * METEOR_TRAIL_WAVER_FRAC * (kf / (n as f32 - 1.0)).powf(1.5);
        let wa = (t * 6.0 + phase + kf * 1.3).sin();
        let wb = (t * 9.4 + phase * 1.7 + kf * 0.7).sin();
        (perp_a * wa + perp_b * wb) * amp
    };

    for (mut transform, mut visibility, segment, material) in trail.iter_mut() {
        let k = segment.index;
        if k >= n || seg_lengths[k] <= 0.0 {
            *visibility = Visibility::Hidden;
            continue;
        }
        *visibility = Visibility::Visible;

        let start = starts[k] + waver_at(k);
        let end = starts[k] + back * seg_lengths[k] + waver_at(k + 1);
        let axis = (end - start).normalize_or_zero();
        let axis = if axis == Vec3::ZERO { back } else { axis };

        // Width: the frustum mesh already bakes the base->apex taper in ball-radius
        // fractions, so scale X/Z by the ball radius (with the head-on flare on the
        // first two segments) and Y by the segment's world length.
        let width = ball_radius * if k < 3 { flare } else { 1.0 };
        transform.translation = start;
        transform.rotation = Quat::from_rotation_arc(Vec3::Y, axis);
        transform.scale = Vec3::new(width, seg_lengths[k], width);

        if let Some(mut material) = materials.get_mut(&material.0) {
            // Intensity falls root -> tip; hue reddens root -> mid -> tip.
            let kt = k as f32 / (n as f32 - 1.0);
            let intensity = lerp(root_intensity, tip_intensity, kt);
            let hue = trail_hue(kt);
            let color = hue * intensity * flicker;
            material.base_color = Color::linear_rgb(color.x, color.y, color.z);
        }
    }
}

/// The reddening trail hue at chain fraction `kt` in `[0, 1]` (0 root, 1 tip):
/// root -> mid at the middle -> tip, lerped linearly.
fn trail_hue(kt: f32) -> Vec3 {
    if kt < 0.5 {
        METEOR_TRAIL_HUE_ROOT.lerp(METEOR_TRAIL_HUE_MID, (kt / 0.5).clamp(0.0, 1.0))
    } else {
        METEOR_TRAIL_HUE_MID.lerp(METEOR_TRAIL_HUE_TIP, ((kt - 0.5) / 0.5).clamp(0.0, 1.0))
    }
}

/// Two unit vectors orthonormal to `axis` (and to each other), for the trail
/// waver's lateral plane. Picks a seed axis not parallel to `axis`.
fn orthonormal_basis(axis: Vec3) -> (Vec3, Vec3) {
    let axis = axis.normalize_or_zero();
    let seed = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let a = seed.cross(axis).normalize_or_zero();
    let b = axis.cross(a).normalize_or_zero();
    (a, b)
}

/// Shed the ember spark stream (and a sparser dark smoke ribbon) behind the
/// fireball at LOD-gated rates, accumulating fractional emissions across frames.
/// Sparks streak in a tight column hugging the tail (not a scattered cloud); the
/// stream ramps up close (`max(descent, tf)`) so dozens of additive sparks only
/// appear in the last seconds. Sized and spread against the RENDERED ball radius
/// so far particles on the grown proxy read at the ball's apparent size and close
/// ones are physical-scale. `f` is the head-on foreshortening factor: it widens
/// the spark spread so sparks streak across the ball's face when the tail is
/// end-on.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn emit_meteor_embers(
    commands: &mut Commands,
    assets: &MeteorEmberAssets,
    emitter: &mut MeteorEmberEmitter,
    event_tick: u64,
    render_pos: Vec3,
    travel: Vec3,
    ball_radius: f32,
    descent: f32,
    tf: f32,
    f: f32,
    dt: f32,
) {
    if emitter.event_tick != event_tick {
        emitter.event_tick = event_tick;
        emitter.spark_accumulator = 0.0;
        emitter.smoke_accumulator = 0.0;
    }
    let dt = dt.max(0.0);
    if dt == 0.0 || travel == Vec3::ZERO || ball_radius <= 0.0 {
        return;
    }

    // Spark rate ramps with the greater of descent / closeness so the dense stream
    // is confined to the close, final-seconds pass; smoke is far sparser.
    let closeness = descent.max(tf).clamp(0.0, 1.0);
    let spark_rate = lerp(METEOR_EMBER_RATE_FAR, METEOR_EMBER_RATE_CLOSE, closeness);
    // Smoke only in the genuine close pass (LOD-gated on `tf`, not descent): far out
    // on the proxy a translucent dark puff reads as an ugly debris dot against the
    // sky, so keep the faint ribbon to when the ball is actually near the camera.
    let smoke_rate = spark_rate * 0.20 * tf;
    emitter.spark_accumulator += spark_rate * dt;
    emitter.smoke_accumulator += smoke_rate * dt;
    // Per-frame caps so a long hitch cannot dump the whole stream at once.
    let sparks = (emitter.spark_accumulator.floor() as u32).min(24);
    let smokes = (emitter.smoke_accumulator.floor() as u32).min(3);
    emitter.spark_accumulator -= sparks as f32;
    emitter.smoke_accumulator -= smokes as f32;

    let back = -travel;
    let (perp_a, perp_b) = orthonormal_basis(back);
    // Ember drift speed unit, off the ball radius so far sparks on the grown proxy
    // move at the ball's apparent rate and close ones at physical rate.
    let ember_unit = ball_radius * 0.18;
    // Widen the lateral spread when the tail is end-on so sparks streak across the
    // ball face rather than piling behind it.
    let lateral = lerp(1.0, 2.2, f);

    for _ in 0..sparks {
        emitter.spawn_seq = emitter.spawn_seq.wrapping_add(1);
        let seq = emitter.spawn_seq;
        let r1 = hashed_unit(seq.wrapping_mul(0x9E37_79B9));
        let r2 = hashed_unit(seq.wrapping_mul(0x85EB_CA6B) ^ 0x27D4_EB2F);
        let r3 = hashed_unit(seq.wrapping_mul(0xC2B2_AE35) ^ 0x1656_67B1);
        let r4 = hashed_unit(seq.wrapping_mul(0x2545_F491) ^ 0x94D0_49BB);

        // A ragged spray strung out along the tail: shed starting a full ball-radius
        // BEHIND the stone (not right at it) and over a long stretch, so the sparks
        // read as a violent river of glowing debris down the tail rather than a bright
        // pile-up right behind the ball (which just washed to cream under AgX).
        let perp = (perp_a * (r2 - 0.5) + perp_b * (r3 - 0.5)) * ball_radius * 0.32 * lateral;
        let offset = back * ball_radius * (1.0 + r1 * 3.0) + perp;
        let velocity = back * ember_unit * (0.9 + r2 * 1.2) + perp * 0.6;
        // Rendered spark radius = ember_mesh(0.12) * initial_scale, so scale off the
        // ball radius directly. Kept smaller (~6-14% of the ball) so each spark stays
        // a distinct high-contrast point that holds its orange, not a fat blob.
        let initial_scale = ball_radius * (0.45 + r3 * 0.7);
        let lifetime = 0.7 + r4 * 0.6;

        commands.spawn((
            Name::new("Meteor Ember"),
            MeteorEmber {
                velocity,
                age: 0.0,
                lifetime,
                initial_scale,
                smoke: false,
            },
            Mesh3d(assets.ember_mesh.clone()),
            MeshMaterial3d(assets.ember_material.clone()),
            Transform::from_translation(render_pos + offset).with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }

    for _ in 0..smokes {
        emitter.spawn_seq = emitter.spawn_seq.wrapping_add(1);
        let seq = emitter.spawn_seq;
        let r1 = hashed_unit(seq.wrapping_mul(0x9E37_79B9));
        let r2 = hashed_unit(seq.wrapping_mul(0x85EB_CA6B) ^ 0x27D4_EB2F);
        let r3 = hashed_unit(seq.wrapping_mul(0xC2B2_AE35) ^ 0x1656_67B1);

        // Smoke sheds further back and a touch below, near-still with slow +Y
        // buoyancy; it grows and fades over life. The 0.5-radius mesh needs only a
        // modest scale.
        let smoke_unit = ball_radius * 0.14;
        let perp = (perp_a * (r2 - 0.5) + perp_b * (r3 - 0.5)) * ball_radius * 0.10;
        let offset = back * ball_radius * (1.0 + r1 * 1.5) - Vec3::Y * ball_radius * 0.15 + perp;
        let velocity = Vec3::Y * smoke_unit * 0.3 + perp * 0.2;
        let initial_scale = smoke_unit * (0.6 + r3 * 0.5);
        let lifetime = 0.8 + r1 * 0.6;

        commands.spawn((
            Name::new("Meteor Smoke"),
            MeteorEmber {
                velocity,
                age: 0.0,
                lifetime,
                initial_scale,
                smoke: true,
            },
            Mesh3d(assets.smoke_mesh.clone()),
            MeshMaterial3d(assets.smoke_material.clone()),
            Transform::from_translation(render_pos + offset).with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }
}

/// Advance and despawn the shed meteor particles in one query pass, branching on
/// the `smoke` flag: sparks fall under gravity with light drag and shrink to a
/// point; smoke drifts up, grows over life, and fades its alpha. World-space, so
/// they linger where shed (the fireball has already moved on). Runs in
/// `ClientSystemSet::Sky`.
pub(crate) fn tick_meteor_ember_system(
    mut commands: Commands,
    time: Res<Time>,
    mut embers: Query<(Entity, &mut Transform, &mut MeteorEmber)>,
) {
    let dt = time.delta_secs().max(0.0);
    if dt == 0.0 {
        return;
    }
    for (entity, mut transform, mut ember) in &mut embers {
        ember.age += dt;
        if ember.age >= ember.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        let life_t = (ember.age / ember.lifetime).clamp(0.0, 1.0);
        if ember.smoke {
            // Smoke: light drift, grow over life. Fade handled per material below.
            ember.velocity *= 1.0 - (0.4 * dt).min(0.9);
            transform.translation += ember.velocity * dt;
            transform.scale = Vec3::splat(ember.initial_scale * (1.0 + 1.5 * life_t));
            // Note: smoke shares one blended material; fading its alpha here would
            // fade every live puff. The short lifetimes + additive fire above keep
            // the ribbon reading as a faint fade without a per-instance clone.
        } else {
            // Spark: gravity + light drag, shrink to a point.
            ember.velocity.y -= METEOR_EMBER_GRAVITY * dt;
            ember.velocity *= 1.0 - (0.6 * dt).min(0.9);
            transform.translation += ember.velocity * dt;
            transform.scale = Vec3::splat((ember.initial_scale * (1.0 - life_t)).max(0.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meteor_render_close_object_in_place_at_true_scale() {
        let camera = Vec3::new(10.0, 2.0, -5.0);
        // Object well within the true-scale distance: drawn where it is, scale 1.
        let true_pos = camera + Vec3::new(0.0, 50.0, 100.0).normalize() * 100.0;
        let (render, scale, tf) = meteor_render_placement(true_pos, camera, 0.95);
        assert!(
            render.distance(true_pos) < 1e-3,
            "a close meteor renders at its true position"
        );
        assert!(
            (scale - 1.0).abs() < 1e-6,
            "and at true (physical) scale regardless of descent"
        );
        assert!(
            (tf - 1.0).abs() < 1e-6,
            "a close meteor is fully in the true-scale LOD (tf = 1)"
        );
    }

    #[test]
    fn meteor_true_scale_factor_is_one_close_and_zero_far() {
        let camera = Vec3::ZERO;
        // Far beyond the proxy distance: tf = 0 (no dense ember stream).
        let far = Vec3::new(0.0, METEOR_PROXY_DISTANCE + 500.0, 0.0);
        let (_, _, tf_far) = meteor_render_placement(far, camera, 0.5);
        assert!((tf_far).abs() < 1e-6, "far meteor has tf = 0, got {tf_far}");
        // Inside the true-scale distance: tf = 1 (full close-pass stream).
        let close = Vec3::new(0.0, METEOR_TRUE_SCALE_DISTANCE - 20.0, 0.0);
        let (_, _, tf_close) = meteor_render_placement(close, camera, 0.5);
        assert!(
            (tf_close - 1.0).abs() < 1e-6,
            "close meteor has tf = 1, got {tf_close}"
        );
        // Midway through the band: strictly between.
        let mid_dist = (METEOR_PROXY_DISTANCE + METEOR_TRUE_SCALE_DISTANCE) * 0.5;
        let mid = Vec3::new(0.0, mid_dist, 0.0);
        let (_, _, tf_mid) = meteor_render_placement(mid, camera, 0.5);
        assert!(
            tf_mid > 0.0 && tf_mid < 1.0,
            "mid-band tf is between 0 and 1, got {tf_mid}"
        );
    }

    #[test]
    fn meteor_far_object_renders_on_proxy_sphere_along_true_bearing() {
        let camera = Vec3::new(0.0, 1.7, 0.0);
        // A far, high object like the entry point: 6 km out, 3 km up.
        let true_pos = camera + Vec3::new(6_000.0, 3_000.0, 0.0);
        let (render, _scale, _tf) = meteor_render_placement(true_pos, camera, 0.0);

        // Rendered exactly on the proxy sphere, along the true direction (so
        // parallax + bearing are preserved), inside the far plane.
        let render_dist = (render - camera).length();
        assert!(
            (render_dist - METEOR_PROXY_DISTANCE).abs() < 1e-2,
            "far meteor renders on the proxy sphere, got {render_dist}"
        );
        assert!(render_dist < 300.0, "and inside the 300 m far plane");
        let true_dir = (true_pos - camera).normalize();
        let render_dir = (render - camera).normalize();
        assert!(
            true_dir.dot(render_dir) > 0.9999,
            "proxy keeps the true bearing so the object stays in the right part of the sky"
        );
    }

    #[test]
    fn meteor_apparent_size_grows_with_descent_on_the_proxy() {
        let camera = Vec3::ZERO;
        // Same far point at two descent fractions: it should be bigger later.
        let true_pos = Vec3::new(4_000.0, 3_000.0, 0.0);
        let (_, entry_scale, _) = meteor_render_placement(true_pos, camera, 0.0);
        let (_, mid_scale, _) = meteor_render_placement(true_pos, camera, 0.5);
        let (_, near_scale, _) = meteor_render_placement(true_pos, camera, 1.0);
        assert!(
            (entry_scale - 1.0).abs() < 1e-6,
            "at entry the proxy ball is the base size, got {entry_scale}"
        );
        assert!(
            mid_scale > entry_scale && near_scale > mid_scale,
            "the ball swells as it descends: {entry_scale} < {mid_scale} < {near_scale}"
        );
        assert!(
            (near_scale - METEOR_PROXY_GROWTH).abs() < 1e-5,
            "by the proxy boundary it has grown to the full growth factor, got {near_scale}"
        );
    }

    #[test]
    fn meteor_placement_blends_smoothly_across_the_handoff_band() {
        let camera = Vec3::ZERO;
        let descent = 1.0;
        // Just outside the proxy distance: full proxy (grown), on the sphere.
        let far = Vec3::new(0.0, METEOR_PROXY_DISTANCE + 5.0, 0.0);
        let (far_pos, far_scale, _) = meteor_render_placement(far, camera, descent);
        assert!((far_pos.length() - METEOR_PROXY_DISTANCE).abs() < 1e-2);
        assert!((far_scale - METEOR_PROXY_GROWTH).abs() < 1e-3);

        // Just inside the true-scale distance: true position + true scale.
        let close = Vec3::new(0.0, METEOR_TRUE_SCALE_DISTANCE - 5.0, 0.0);
        let (close_pos, close_scale, _) = meteor_render_placement(close, camera, descent);
        assert!(close_pos.distance(close) < 1e-3);
        assert!((close_scale - 1.0).abs() < 1e-6);

        // Midway through the band: between the two, monotone in both.
        let mid_dist = (METEOR_PROXY_DISTANCE + METEOR_TRUE_SCALE_DISTANCE) * 0.5;
        let mid = Vec3::new(0.0, mid_dist, 0.0);
        let (_, mid_scale, _) = meteor_render_placement(mid, camera, descent);
        assert!(
            mid_scale < far_scale && mid_scale > close_scale,
            "scale eases from grown-proxy to true across the band: {far_scale} > {mid_scale} > {close_scale}"
        );
    }
}
