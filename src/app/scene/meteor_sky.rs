//! Airborne meteor shower VFX: the burning-rock fireball body, the animated
//! flame tongues licking off it, its segmented fiery trail, the ember/smoke
//! stream it sheds in flight, and the one-time mid-flight airburst (a bolide
//! fragmentation flash that fans out small burning fragments which sputter and
//! burn up in the sky).
//!
//! This is the SKY half of the meteor shower event; the ground half (crater,
//! site fires, rock blast, strike cues) lives in `scene::meteor_shower`. The
//! fireball is a true world-anchored object, placed each frame from the shared
//! deterministic trajectory (`crate::world::meteor_shower`) evaluated against
//! the local clock estimate; see [`MeteorVisual`] and
//! [`update_meteor_sky_system`] for the far-plane proxy scheme that keeps it
//! renderable and correctly sized from kilometres out. The body, tongue, and
//! trail entities are spawned once by [`setup_meteor_sky`] (called from
//! `setup_scene` right after the sky rig) and repositioned/reshaded per frame;
//! the shed embers and airburst fragments are short-lived world-space
//! particles. Everything here is presentation only: the authoritative meteor
//! (arc, impact, blast) is untouched by the airburst, the main fireball simply
//! flies on as the surviving core.

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

/// How many fireball rigs the sky pre-spawns. One per possible concurrent
/// meteor: a shower rolls at most `METEOR_SHOWER_COUNT_MAX` meteors, and the
/// impact staggering keeps several in flight at once, so every meteor needs
/// its own body/trail/tongue set. Const-asserted against the balance knob so a
/// count bump cannot silently starve the sky of rigs.
pub(crate) const METEOR_SKY_RIGS: usize = 5;
const _: () = assert!(
    crate::game_balance::METEOR_SHOWER_COUNT_MAX as usize <= METEOR_SKY_RIGS,
    "pre-spawned sky rigs must cover the maximum shower size"
);

/// Which pre-spawned fireball rig an entity belongs to. Every body layer,
/// trail segment, and flame tongue carries one; the update system assigns
/// meteor `k` of the live shower to rig `k` and drives each rig's entities
/// from its own meteor's trajectory.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeteorRig(pub(crate) usize);

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
    /// Thin additive HDR GLOW RIM just past the shell: a bright annulus whose
    /// bloom is what makes the fireball read as LUMINOUS. The deep opaque shell
    /// carries the saturated colour mass; this rim carries the light. Thin on
    /// purpose: a bright additive area this narrow blooms into a hot aura
    /// without tonemapping the whole ball to cream.
    Glow,
}

/// How a body layer renders. See the `body_layer` builder in `setup_meteor_sky`
/// for the per-mode reasoning (irregular dark rock / additive flame halo /
/// additive cap / additive glow rim).
#[derive(Clone, Copy)]
enum MeteorBodyRender {
    /// Opaque, default back-face cull, IRREGULAR mesh: the solid dark rock heart.
    OpaqueRock,
    /// Opaque, FRONT-face cull: the far bowl showing a saturated orange flame ring.
    OpaqueRing,
    /// Additive, back-face cull: the small white-hot leading cap.
    AdditiveCap,
    /// Additive, FRONT-face cull: the far bowl of a sphere slightly larger than
    /// the shell. The opaque shell's far wall depth-occludes everything inside
    /// its own silhouette, so only the thin annulus between the shell rim and
    /// this sphere's rim survives: a narrow blazing ring that blooms.
    AdditiveRing,
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
/// that drifts at its fixed spawn size and despawns. One component so
/// [`tick_meteor_ember_system`] stays a single query pass.
#[derive(Component)]
pub(crate) struct MeteorEmber {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    /// `true` for a smoke puff (buoyant drift), `false` for a spark (gravity +
    /// light drag). Neither animates scale.
    smoke: bool,
}

/// One animated flame tongue licking around the fireball shell: a small opaque
/// unlit ellipsoid anchored on the rock's rim, orbiting the travel axis slowly
/// and flaring backward on its own seeded pulse. A co-moving TOP-LEVEL world
/// entity like the trail segments (parenting under [`MeteorVisual`] renders
/// invisible in this Bevy version), repositioned each frame by
/// [`update_meteor_flame_tongues`]. The tongues are what make the distant
/// slow-crossing phase read as a thing on FIRE rather than a static glowing
/// dot: the silhouette constantly sprouts and swallows little flame spikes.
#[derive(Component)]
pub(crate) struct MeteorFlameTongue {
    index: usize,
}

/// One small burning fragment flung out by the mid-flight airburst: an additive
/// hot blob elongated along its own velocity that arcs away from the core,
/// sputters [`MeteorEmber`] sparks behind it, and burns up (shrinks away) well
/// before reaching the ground. World-space and self-despawning; advanced by
/// [`tick_meteor_airburst_system`].
#[derive(Component)]
pub(crate) struct MeteorFragment {
    velocity: Vec3,
    age: f32,
    lifetime: f32,
    initial_scale: f32,
    /// Downward pull (m/s^2), pre-scaled to the rendered ball size at burst so
    /// proxy-space fragments arc at the ball's apparent rate.
    gravity: f32,
    /// Countdown (seconds) to the next shed ember spark.
    ember_cooldown: f32,
    /// Free-running draw seed salting each shed ember so the sputter varies.
    seed: u32,
    /// Age at the moment the fragment reached the ground plane, or `None`
    /// while airborne. A grounded fragment rests in place (no more spin,
    /// no more embers) for a beat before despawning: solid matter lands and
    /// stays, it never burns away mid-air or sinks through the floor.
    grounded_at: Option<f32>,
}

/// A one-shot additive flash at a FIXED size that fades its emissive color to
/// black over `lifetime`, then despawns. Owns its material instance (created at
/// spawn) so the per-frame fade never touches a shared handle. Used by the
/// mid-flight airburst core pop and the ground-zero impact fireball; neither
/// animates scale (owner call: no particle ever transforms in size, and the old
/// growing/shrinking flash domes read as huge scale-animating particles).
#[derive(Component)]
pub(crate) struct MeteorAirburstFlash {
    age: f32,
    lifetime: f32,
    /// Peak emissive color (linear); the fade multiplies this down to black.
    color: Vec3,
}

impl MeteorAirburstFlash {
    /// Crate-visible so the impact-site VFX (`super::meteor_shower`) can spawn
    /// its ground fireball through the same fade ticker.
    pub(crate) fn new(lifetime: f32, color: Vec3) -> Self {
        Self {
            age: 0.0,
            lifetime,
            color,
        }
    }
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

/// Descent window over which the proxy's DRAMA growth converges back to the
/// TRUE apparent size (angular-size-preserving: mesh scale = proxy distance /
/// true distance). The camera-distance blend above only helps observers the
/// meteor actually approaches; a player watching a far impact used to keep the
/// fully-grown proxy sprite all the way to the ground, where it just vanished.
/// Converging on descent instead means every observer watches the giant far
/// glow settle into the real few-metre object diving onto the site, matching
/// the crater it leaves.
const METEOR_TRUE_SIZE_CONVERGE_START: f32 = 0.70;
const METEOR_TRUE_SIZE_CONVERGE_END: f32 = 0.96;

/// Distance band over which the proxy's drama growth is DAMPED for meteors
/// that stay far from this camera. The full `METEOR_PROXY_GROWTH` swell is the
/// bearing-down read and only makes sense for a meteor actually coming at you;
/// a strike landing on the far side of the map used to swell just as huge in
/// the sky, which read wrong (owner report). Inside `FULL` the drama is
/// untouched; by `FAR` only `KEEP` of the growth term survives, so a distant
/// crossing is a modest bright streak, still clearly visible (and, via the
/// audio floor, audible) but plausibly far. Keyed off the CURRENT camera
/// distance, so a near-site meteor regains its full swell as it closes.
const METEOR_DRAMA_FULL_DISTANCE_M: f32 = 800.0;
const METEOR_DRAMA_FAR_DISTANCE_M: f32 = 2_200.0;
const METEOR_DRAMA_FAR_KEEP: f32 = 0.15;

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
// The ENTRY value is deeper still: on the distant slow phase the ring IS most
// of the fireball's area, and at 1.35 red it tonemapped to a pale tan donut at
// noon (screenshot-confirmed); darker holds the saturated ember-red. It lerps
// up to the hotter impact value as the ball dives, so the close pass keeps its
// brighter flame.
const METEOR_SHELL_ENTRY: Vec3 = Vec3::new(0.85, 0.09, 0.0);
const METEOR_SHELL_IMPACT: Vec3 = Vec3::new(1.9, 0.22, 0.0);
/// ADDITIVE white-hot leading cap linear base_color at entry / impact. Hotter than
/// the halo (high red + strong green so it reads white-yellow, the hottest part of
/// the fire, with a little blue so the very core tips toward white without going
/// cyan) but kept MODERATE so it stays a saturated incandescent nose, not a flat
/// white disc that swallows the rock. Sits over the rock's nose only.
const METEOR_CORONA_ENTRY: Vec3 = Vec3::new(1.5, 0.52, 0.05);
const METEOR_CORONA_IMPACT: Vec3 = Vec3::new(2.2, 0.80, 0.10);
/// GLOW rim sphere radius fraction: a HAIR past the shell, so the additive rim
/// survives depth-testing only as a thin annulus between the two silhouettes.
/// Thin is the whole trick: at 0.72 the annulus was as wide as the shell ring
/// and tonemapped to yet another flat tan band (screenshot-confirmed); a
/// narrow line stays a crisp blazing edge that bloom spreads into a halo.
const METEOR_GLOW_RADIUS_FRAC: f32 = 0.665;
/// ADDITIVE glow-rim linear base_color at entry / impact. HDR-bright on purpose:
/// this is the layer that feeds bloom so the fireball actually GLOWS instead of
/// reading as flat stacked sprites. It can afford to be bright where the shell
/// cannot because the visible area is a narrow ring, so AgX never sees a large
/// bright patch to bleach.
const METEOR_GLOW_ENTRY: Vec3 = Vec3::new(3.2, 0.70, 0.04);
const METEOR_GLOW_IMPACT: Vec3 = Vec3::new(4.5, 1.10, 0.08);

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
/// apparent size from any distance.) The entry length is generous too, so the
/// distant slow-crossing phase already drags a real fire streak.
const METEOR_TRAIL_LENGTH_ENTRY: f32 = 10.0;
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
/// clearly visible even mid-flight, not just in the final seconds; raised again
/// so the distant slow phase reads as actively burning, not a quiet dot).
const METEOR_EMBER_RATE_FAR: f32 = 130.0;
const METEOR_EMBER_RATE_CLOSE: f32 = 240.0;
/// Downward acceleration (m/s^2) applied to shed sparks so they arc off the tail.
const METEOR_EMBER_GRAVITY: f32 = 9.0;

// Far-phase fire. Early in the flight the fireball crosses the sky slowly and
// far out, and the calm shared flicker (+/-8%) made it read as a static glowing
// disc rather than a burning object. Two renderer-side treatments fix that
// without touching the shared deterministic trajectory: the flame TONGUES (small
// opaque licks dancing around the shell, see [`MeteorFlameTongue`]) and a far
// AMPLIFICATION of the shared flicker, easing back to the calm value for the
// close pass (a +/-20% scale wobble on a screen-filling ball would strobe).

/// How many flame tongues lick around the fireball shell.
const METEOR_FLAME_TONGUE_COUNT: usize = 7;
/// Tongue hue at full flare / at rest (linear HDR). DEEP saturated fire: under
/// AgX at noon even the trail-root value (0.95 red) drifts tan on shapes this
/// size, and the darker the value the more saturation survives (the first
/// tongue pass at 1.5 red read as dough lumps, screenshot-confirmed; 0.95 was
/// still salmon). These sit low enough to hold a genuine ember-red by day and
/// glow saturated orange against the night sky.
const METEOR_TONGUE_HUE_FLARE: Vec3 = Vec3::new(0.70, 0.075, 0.0);
const METEOR_TONGUE_HUE_REST: Vec3 = Vec3::new(0.34, 0.020, 0.0);
/// Per-vertex multipliers baked into the tongue mesh's COLOR_0, root -> tip
/// along local Y. The ROOT (the end buried in the ball) multiplies the deep
/// material hue up into HDR hot orange, so each lick has a small incandescent
/// base that blooms and fades along its own length toward the deep-red tip.
/// This gradient is what stops a tongue reading as one flat solid sprite: the
/// flame glows where it leaves the fire and cools along its length.
const METEOR_TONGUE_ROOT_MULT: Vec3 = Vec3::new(3.2, 5.0, 1.0);
const METEOR_TONGUE_TIP_MULT: Vec3 = Vec3::new(0.55, 0.40, 1.0);
/// Tongue prominence multiplier at entry / impact. Full drama on the distant
/// slow phase (where the licks carry the "on fire" read); scaled well down for
/// the close pass, whose look (rock + shell + cap + trail + dense embers) was
/// already right without them.
const METEOR_TONGUE_PROMINENCE_ENTRY: f32 = 1.0;
const METEOR_TONGUE_PROMINENCE_IMPACT: f32 = 0.45;
/// Multiplier on the shared flicker's amplitude at the far/entry extreme; 1.0
/// (the calm shared value) at the close extreme.
const METEOR_FAR_SHIMMER_BOOST: f32 = 2.8;

// Mid-flight airburst. Real bolides often fragment before impact: partway
// through the descent the fireball pops a brief hot flash and fans out a
// handful of small burning fragments that scatter from the core, sputter
// embers, and burn up in the sky long before the ground. Pure presentation,
// deterministic off the trajectory seed; the authoritative meteor and the main
// fireball are untouched (the ball flies on as the surviving core).

/// Fraction of the VISIBLE flight (from the first frame THIS client saw the
/// fireball, to impact) after which the airburst pops. Anchoring on the watched
/// window rather than a fixed descent fraction means a short-warning event
/// (`/meteor-here`, 8 s) still gets its burst a few seconds in, right as the
/// object kicks into its fast dive, instead of the trigger point having
/// silently passed before the fireball ever appeared. For the standard event
/// watched from the announce, the visible window IS the whole flight, so this
/// stays "halfway down".
const METEOR_AIRBURST_VISIBLE_FRACTION: f32 = 0.5;
/// Grace window past the trigger fraction inside which an observed crossing
/// still fires (a hitch or brief menu straddling the moment). Beyond it the
/// burst is skipped entirely so a stale pop never replays.
const METEOR_AIRBURST_WINDOW: f32 = 0.06;
/// How many burning fragments the airburst fans out.
const METEOR_AIRBURST_FRAGMENTS: u32 = 9;
/// One-off ember sparks flung radially at the burst instant.
const METEOR_AIRBURST_SPARKS: u32 = 26;
/// Flash lifetime in seconds and peak radius in rendered ball radii.
const METEOR_AIRBURST_FLASH_SECONDS: f32 = 0.45;
const METEOR_AIRBURST_FLASH_PEAK_FRAC: f32 = 2.5;
/// Additive flash colour at full brightness (linear HDR). Held DEEP (a broad
/// additive area brighter than this tonemaps to a tan disc, not fire); it
/// fades to nothing over the flash lifetime.
const METEOR_AIRBURST_FLASH_COLOR: Vec3 = Vec3::new(1.9, 0.32, 0.02);
/// Opaque fragment hues (linear HDR), alternated across the fan: an ember
/// orange and a deeper ember red. Both kept LOW: the first pass on the shared
/// bright-additive ember material washed to cream petals, and even opaque
/// 0.95-red drifted salmon at noon; of the two test hues only the darker one
/// held its red (screenshot-confirmed), so both now sit in that range.
const METEOR_FRAGMENT_HUE_HOT: Vec3 = Vec3::new(0.70, 0.075, 0.0);
const METEOR_FRAGMENT_HUE_DEEP: Vec3 = Vec3::new(0.42, 0.025, 0.0);
/// Seconds between ember sparks shed by one burning fragment.
const METEOR_FRAGMENT_EMBER_INTERVAL: f32 = 0.07;
/// How long a landed airburst fragment rests on the ground before despawning.
const METEOR_FRAGMENT_REST_SECONDS: f32 = 2.5;

/// Spawn the meteor shower fireball rigs (one per possible concurrent meteor,
/// [`METEOR_SKY_RIGS`]) and their trail chains, all `Hidden` until an event is
/// in flight. Called from `setup_scene` right after `setup_sky` so the meteor
/// rigs spawn alongside the rest of the sky.
///
/// Each fireball is three co-moving unlit fog-immune meshes placed at a TRUE
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
    for rig in 0..METEOR_SKY_RIGS {
        setup_meteor_rig(commands, meshes, materials, rig);
    }
}

/// Spawn one fireball rig (body layers, trail chain, flame tongues), every
/// entity tagged [`MeteorRig`] so the update system can drive each rig from
/// its own meteor.
fn setup_meteor_rig(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    rig: usize,
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
        // is a jagged stone, not a smooth ball (seeded per rig so concurrent
        // meteors are not clones); the two fire layers stay smooth spheres (the
        // additive glow does not want facets).
        let radius = METEOR_BASE_RADIUS * radius_frac;
        let mesh = match render {
            MeteorBodyRender::OpaqueRock => meshes.add(irregular_rock_mesh(
                radius,
                0x1234_5678 ^ (rig as u32).wrapping_mul(0x9E37_79B9),
            )),
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
        //  - `AdditiveRing`: the glow rim. Additive, FRONT-face cull, on a sphere just
        //    larger than the shell: the shell's opaque far wall depth-hides everything
        //    inside its own silhouette, leaving a THIN blazing annulus that feeds
        //    bloom, which is where the fireball's "it glows" read comes from.
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
            MeteorBodyRender::AdditiveRing => (
                AlphaMode::Add,
                Some(bevy::render::render_resource::Face::Front),
            ),
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
            Name::new(format!("{name} {rig}")),
            MeteorVisual,
            MeteorRig(rig),
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
        MeteorBodyLayer::Glow,
        "Meteor Glow Rim",
        METEOR_GLOW_RADIUS_FRAC,
        METEOR_GLOW_ENTRY,
        MeteorBodyRender::AdditiveRing,
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
            Name::new(format!("Meteor Trail Segment {rig}.{index}")),
            MeteorTrailSegment { index },
            MeteorRig(rig),
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            NotShadowCaster,
        ));
    }

    // Flame tongues: small OPAQUE unlit ellipsoids (a shared unit sphere,
    // stretched by the per-frame transform) that dance around the shell rim.
    // Opaque for the same AgX reason as the trail cones: additive licks over a
    // bright day sky wash to cream, an opaque deep-orange lump occludes the sky
    // and keeps its fire hue. The mesh bakes a COLOR_0 gradient (incandescent
    // root -> deep tip) so each lick glows where it leaves the fire and cools
    // along its length instead of reading as one flat solid sprite. Each tongue
    // owns its material so the per-frame pulse can shade it independently.
    let tongue_mesh = meshes.add(flame_tongue_mesh());
    for index in 0..METEOR_FLAME_TONGUE_COUNT {
        let material = materials.add(StandardMaterial {
            base_color: Color::linear_rgb(
                METEOR_TONGUE_HUE_FLARE.x,
                METEOR_TONGUE_HUE_FLARE.y,
                METEOR_TONGUE_HUE_FLARE.z,
            ),
            unlit: true,
            fog_enabled: false,
            ..default()
        });
        commands.spawn((
            Name::new(format!("Meteor Flame Tongue {rig}.{index}")),
            MeteorFlameTongue { index },
            MeteorRig(rig),
            Mesh3d(tongue_mesh.clone()),
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

/// Build the shared flame-tongue mesh: a unit ico-sphere with a COLOR_0
/// gradient along local Y, from [`METEOR_TONGUE_ROOT_MULT`] at `y = -1` (the
/// root end, buried in the ball) to [`METEOR_TONGUE_TIP_MULT`] at `y = +1`
/// (the tip). The standard material multiplies vertex colour into the base
/// colour, so the per-frame deep hue gets an incandescent HDR root (small
/// enough to bloom without washing) fading to a deep-red tip along each lick.
fn flame_tongue_mesh() -> Mesh {
    use bevy::mesh::VertexAttributeValues;

    let mut mesh = Sphere::new(1.0).mesh().ico(2).expect("valid subdivisions");
    let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)
    else {
        return mesh;
    };
    let colors: Vec<[f32; 4]> = positions
        .iter()
        .map(|p| {
            // t = 0 at the root (y = -1), 1 at the tip (y = +1), smoothed so
            // the hot zone hugs the root instead of bleeding halfway up.
            let t = ((p[1] + 1.0) * 0.5).clamp(0.0, 1.0);
            let t = t * t * (3.0 - 2.0 * t);
            let m = METEOR_TONGUE_ROOT_MULT.lerp(METEOR_TONGUE_TIP_MULT, t);
            [m.x, m.y, m.z, 1.0]
        })
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
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
        &'static MeteorRig,
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
        &'static MeteorRig,
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

type MeteorTongueQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Visibility,
        &'static MeteorFlameTongue,
        &'static MeteorRig,
        &'static MeshMaterial3d<StandardMaterial>,
    ),
    (
        Without<MeteorVisual>,
        Without<MeteorTrailSegment>,
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

    // Apparent size on the proxy: grows with descent so the ball bears down,
    // then CONVERGES back to the true angular size over the final stretch of
    // descent (see the converge constants) so a far observer watches the grown
    // glow settle into the real object diving onto the site rather than a huge
    // sprite that never transitions and simply vanishes at impact. The growth
    // is distance-damped (the drama-damp constants): a meteor that stays far
    // from THIS camera swells only a fraction, so cross-map strikes read as
    // plausibly distant streaks instead of looming overhead.
    let far_t = ((distance - METEOR_DRAMA_FULL_DISTANCE_M)
        / (METEOR_DRAMA_FAR_DISTANCE_M - METEOR_DRAMA_FULL_DISTANCE_M))
        .clamp(0.0, 1.0);
    let drama_damp = 1.0 - far_t * (1.0 - METEOR_DRAMA_FAR_KEEP);
    let drama_scale = 1.0 + descent.clamp(0.0, 1.0) * (METEOR_PROXY_GROWTH - 1.0) * drama_damp;
    // Mesh drawn at the proxy distance has the true apparent size when scaled
    // by the distance ratio. Clamped so a degenerate close distance cannot
    // explode the scale before the band blend takes over.
    let true_apparent_scale = (METEOR_PROXY_DISTANCE / distance).min(METEOR_PROXY_GROWTH);
    let converge = ((descent - METEOR_TRUE_SIZE_CONVERGE_START)
        / (METEOR_TRUE_SIZE_CONVERGE_END - METEOR_TRUE_SIZE_CONVERGE_START))
        .clamp(0.0, 1.0);
    let converge = converge * converge * (3.0 - 2.0 * converge);
    let proxy_scale = drama_scale + (true_apparent_scale - drama_scale) * converge;
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

/// Airburst bookkeeping: which event the flags belong to, the descent at which
/// this client FIRST saw the fireball (anchors the trigger point to the watched
/// window), the descent observed on the previous in-flight frame, and whether
/// the one-shot burst has fired.
#[derive(Default)]
pub(crate) struct MeteorAirburstState {
    event_tick: u64,
    first_seen_descent: Option<f32>,
    prev_descent: Option<f32>,
    fired: bool,
}

/// The descent fraction at which the airburst pops for a client that first saw
/// the fireball at `first_seen_descent`: [`METEOR_AIRBURST_VISIBLE_FRACTION`]
/// of the way through the flight it actually gets to watch. A full-flight
/// viewer bursts halfway down; a short `/meteor-here` viewer bursts halfway
/// through its brief dive. Pure so the timing is unit-testable.
fn airburst_threshold(first_seen_descent: f32) -> f32 {
    let first_seen = first_seen_descent.clamp(0.0, 1.0);
    first_seen + METEOR_AIRBURST_VISIBLE_FRACTION * (1.0 - first_seen)
}

/// Whether the airburst fires this frame, given the trigger threshold and the
/// descent observed on the previous in-flight frame (`None` if this is the
/// first observed frame of the event). Only a WATCHED crossing inside the
/// grace window fires; a client returning from a long menu straddle past the
/// window skips the pop entirely rather than replaying it stale. Pure so the
/// trigger is unit-testable.
fn airburst_crossing(prev_descent: Option<f32>, descent: f32, threshold: f32) -> bool {
    let Some(prev) = prev_descent else {
        return false;
    };
    prev < threshold && (threshold..threshold + METEOR_AIRBURST_WINDOW).contains(&descent)
}

/// Hide trail segments (used from the fireball's early-return / not-in-flight
/// paths so a stale streak never lingers on the sky). `rig` limits the hide to
/// one rig; `None` hides every rig.
fn set_trail_hidden(trail: &mut MeteorTrailQuery, rig: Option<usize>) {
    for (_, mut visibility, _, rig_tag, _) in trail.iter_mut() {
        if rig.is_none_or(|rig| rig_tag.0 == rig) {
            *visibility = Visibility::Hidden;
        }
    }
}

/// Hide fireball body layers (core/shell/corona/glow) on the not-in-flight
/// paths. `rig` limits the hide to one rig; `None` hides every rig.
fn set_body_hidden(body: &mut MeteorVisualQuery, rig: Option<usize>) {
    for (_, mut visibility, _, rig_tag, _) in body.iter_mut() {
        if rig.is_none_or(|rig| rig_tag.0 == rig) {
            *visibility = Visibility::Hidden;
        }
    }
}

/// Hide flame tongues on the not-in-flight paths. `rig` limits the hide to one
/// rig; `None` hides every rig.
fn set_tongues_hidden(tongues: &mut MeteorTongueQuery, rig: Option<usize>) {
    for (_, mut visibility, _, rig_tag, _) in tongues.iter_mut() {
        if rig.is_none_or(|rig| rig_tag.0 == rig) {
            *visibility = Visibility::Hidden;
        }
    }
}

/// Position, orient, size, and shade every live meteor's fireball each frame
/// from the shared deterministic **world-space** trajectory
/// (`crate::world::meteor_shower::meteor_world_state`) evaluated against the
/// local clock estimate, and shed each fireball's ember sputter stream behind
/// it. Meteor `k` of the shower drives rig `k` ([`MeteorRig`]); rigs without a
/// live in-flight meteor stay hidden.
///
/// Each object is a true world entity: the far-plane proxy
/// ([`meteor_render_placement`]) keeps it renderable and correctly sized from
/// any distance while preserving parallax, so players can follow it from a
/// distant burning point all the way to a scream-overhead landing. A meteor's
/// `size` multiplies the rendered ball radius, so every dependent effect
/// (trail length, tongues, embers, airburst) scales with it for free. The
/// trail entities' local -Z is aligned with the (analytic, stable) velocity so
/// the streak drags straight behind travel, and the materials' HDR brightness
/// is rewritten per frame (descent ramp x seeded flicker) so each ball visibly
/// burns against day and night skies alike. Runs in `ClientSystemSet::Sky`
/// alongside `update_sky_system`; gated on `!uses_menu_backdrop` (the title
/// screen has no world) per gotcha 12.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn update_meteor_sky_system(
    mut commands: Commands,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    time: Res<Time>,
    ember_assets: Option<Res<MeteorEmberAssets>>,
    mut emitters: Local<[MeteorEmberEmitter; METEOR_SKY_RIGS]>,
    mut airbursts: Local<[MeteorAirburstState; METEOR_SKY_RIGS]>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera: CameraTransformQuery,
    mut meteor: MeteorVisualQuery,
    mut trail: MeteorTrailQuery,
    mut tongues: MeteorTongueQuery,
) {
    // Resolve the live shower, or hide every rig. Hidden when: no event or the
    // title backdrop is up (per-rig hiding below covers not-in-flight meteors).
    if menu.screen.uses_menu_backdrop() || runtime.meteor_showers.is_empty() {
        set_body_hidden(&mut meteor, None);
        set_trail_hidden(&mut trail, None);
        set_tongues_hidden(&mut tongues, None);
        return;
    }
    let Ok(camera_transform) = camera.single() else {
        set_body_hidden(&mut meteor, None);
        set_trail_hidden(&mut trail, None);
        set_tongues_hidden(&mut tongues, None);
        return;
    };
    let camera_pos = camera_transform.translation;
    let camera_forward = camera_transform.forward().as_vec3();
    // The FRACTIONAL clock estimate: evaluating a committed arc at whole 20 Hz
    // ticks quantises the plunge into 50 ms position steps, which reads as a
    // stuttering final descent on any client rendering faster than the tick rate.
    let now = runtime.server_tick_precise();

    for rig in 0..METEOR_SKY_RIGS {
        let event = runtime.meteor_showers.get(rig).copied();
        let state = event.and_then(|event| {
            crate::world::meteor_world_state(
                Vec2::new(event.impact_position.x, event.impact_position.z),
                event.impact_tick,
                event.trajectory_seed,
                now,
            )
        });
        let (Some(event), Some(state)) = (event, state) else {
            // No meteor assigned to this rig, or its meteor is not in flight
            // (pre-entry, or already struck): hide just this rig.
            set_body_hidden(&mut meteor, Some(rig));
            set_trail_hidden(&mut trail, Some(rig));
            set_tongues_hidden(&mut tongues, Some(rig));
            continue;
        };
        update_meteor_rig_frame(
            &mut commands,
            &mut materials,
            &mut meteor,
            &mut trail,
            &mut tongues,
            ember_assets.as_deref(),
            &mut emitters[rig],
            &mut airbursts[rig],
            rig,
            &event,
            &state,
            camera_pos,
            camera_forward,
            now,
            time.delta_secs(),
        );
    }
}

/// Drive one rig's body layers, trail, tongues, airburst, and ember stream for
/// one frame from its meteor's in-flight state. Split out of
/// [`update_meteor_sky_system`] so the per-rig loop stays legible.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn update_meteor_rig_frame(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meteor: &mut MeteorVisualQuery,
    trail: &mut MeteorTrailQuery,
    tongues: &mut MeteorTongueQuery,
    ember_assets: Option<&MeteorEmberAssets>,
    emitter: &mut MeteorEmberEmitter,
    airburst: &mut MeteorAirburstState,
    rig: usize,
    event: &crate::app::state::MeteorShowerEvent,
    state: &crate::world::MeteorWorldState,
    camera_pos: Vec3,
    camera_forward: Vec3,
    now: f64,
    dt: f32,
) {
    let descent = state.descent_fraction;
    let flicker = state.flicker;
    // The meteor's size scales the whole rendered fireball; every dependent
    // effect below is sized off `ball_radius`, so trail/tongues/embers/airburst
    // inherit the scale for free.
    let size = event.size.clamp(0.05, 1.0);

    // Place on the far-plane proxy (growing with descent) or, once close, at the
    // true world position and true scale. Preserves parallax and lets the object
    // be followed from a distant burning point to a scream-overhead landing. `tf`
    // is the close-ness LOD factor that gates the dense ember stream.
    let (render_pos, render_scale, tf) =
        meteor_render_placement(state.position, camera_pos, descent);
    let ball_radius = METEOR_BASE_RADIUS * size * render_scale;

    // Far-phase shimmer: widen the shared deterministic flicker while the ball
    // is distant + early in its flight, so the slow-crossing phase visibly
    // sputters and burns rather than gliding as a static disc, easing back to
    // the calm shared amplitude for the close pass.
    let far_fire = ((1.0 - descent) * (1.0 - tf)).clamp(0.0, 1.0);
    let shimmer = 1.0 + (flicker - 1.0) * lerp(1.0, METEOR_FAR_SHIMMER_BOOST, far_fire);

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
    // Scale wobbles only with the CALM shared flicker; the amplified far
    // shimmer drives brightness alone. Pumping the amplified value into the
    // mesh scale made the whole rig visibly inflate and deflate, which read as
    // a pulsating sprite rather than a burning object. The meteor's size folds
    // in here (the meshes are built at the size-1.0 base radius).
    let s = render_scale * flicker * size;
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
    for (mut transform, mut visibility, layer, rig_tag, material) in meteor.iter_mut() {
        if rig_tag.0 != rig {
            continue;
        }
        *visibility = Visibility::Visible;
        transform.rotation = rotation;
        transform.scale = body_scale;
        transform.translation = render_pos;
        let (entry, impact, heat_t) = match layer {
            MeteorBodyLayer::Core => {
                // The stone tumbles (about its own centre) inside the fixed halo.
                transform.rotation = rotation * tumble;
                (METEOR_CORE_ENTRY, METEOR_CORE_IMPACT, descent)
            }
            // The shell's heat ramp is EASED (descent cubed): a linear lerp
            // put the ring halfway to the bright impact value by mid-flight,
            // where AgX washed it back to the tan donut the deep entry colour
            // exists to avoid (squared still drifted pale by mid-flight).
            // Cubing keeps the whole crossing ember-red and saves the hot
            // flame for the final plunge.
            MeteorBodyLayer::Shell => (
                METEOR_SHELL_ENTRY,
                METEOR_SHELL_IMPACT,
                descent * descent * descent,
            ),
            MeteorBodyLayer::Glow => (METEOR_GLOW_ENTRY, METEOR_GLOW_IMPACT, descent),
            MeteorBodyLayer::Corona => {
                // Leading cap: shove it forward over the nose so the incandescent
                // shock front sits on the travel-forward face, not the centre.
                transform.translation = render_pos + cap_offset;
                (METEOR_CORONA_ENTRY, METEOR_CORONA_IMPACT, descent)
            }
        };
        if let Some(mut material) = materials.get_mut(&material.0) {
            let color = entry.lerp(impact, heat_t.clamp(0.0, 1.0)) * shimmer;
            material.base_color = Color::linear_rgb(color.x, color.y, color.z);
        }
    }

    // Trail: a long tapering fiery streak dragged straight behind the ball, built
    // from a chain of frustum segments (world entities, not children). Foreshorten-
    // ing flare + a bounded lateral waver make it read as a comet from any pose.
    update_meteor_trail(
        trail,
        materials,
        rig,
        render_pos,
        travel,
        camera_forward,
        ball_radius,
        descent,
        shimmer,
        event.trajectory_seed,
        now,
        camera_pos,
    );

    // Flame tongues: small opaque licks dancing around the shell rim, the "it
    // is on fire" read for the distant slow phase (and extra flame mass close).
    update_meteor_flame_tongues(
        tongues,
        materials,
        rig,
        render_pos,
        travel,
        ball_radius,
        descent,
        shimmer,
        event.trajectory_seed,
        now,
    );

    // Mid-flight airburst: fire the one-shot fragmentation on a watched crossing
    // of the trigger point, anchored to when THIS client first saw the fireball
    // (so a short `/meteor-here` dive still bursts partway in). Spawned at the
    // RENDER position (like the embers) so the pop sits on the visible ball
    // whether it is on the proxy or close.
    if airburst.event_tick != event.impact_tick {
        airburst.event_tick = event.impact_tick;
        airburst.first_seen_descent = None;
        airburst.prev_descent = None;
        airburst.fired = false;
    }
    let first_seen = *airburst.first_seen_descent.get_or_insert(descent);
    let prev_descent = airburst.prev_descent.replace(descent);
    if !airburst.fired && airburst_crossing(prev_descent, descent, airburst_threshold(first_seen)) {
        airburst.fired = true;
        if let Some(ember_assets) = ember_assets {
            spawn_meteor_airburst(
                commands,
                ember_assets,
                materials,
                render_pos,
                travel,
                ball_radius,
                (1.0 - descent).max(0.0) * crate::world::METEOR_FLIGHT_SECONDS,
                event.trajectory_seed,
                size,
            );
        }
    }

    // Ember + smoke stream: shed at the fireball's RENDER position (so they read as
    // coming off the visible ball), each a world-space particle so it lingers and
    // drifts where shed. The dedicated bright ember material (not the dim torch
    // flame) holds orange in daylight; the stream is LOD-gated to the close pass so
    // dozens of additive sparks only appear in the last seconds.
    if let Some(ember_assets) = ember_assets {
        // Foreshortening factor: 0 side-on, 1 head-on (tail pointing at camera).
        let f = foreshortening_factor(travel, camera_forward);
        emit_meteor_embers(
            commands,
            ember_assets,
            emitter,
            event.impact_tick,
            render_pos,
            travel,
            ball_radius,
            descent,
            tf,
            f,
            size,
            dt,
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

/// Position, orient, size, and shade one rig's trail segments for one frame. The
/// chain walks straight back from the ball along `-travel`, each segment a frustum
/// scaled to its share of the total tail length, with a bounded lateral waver and
/// a root-flare for the head-on pose. Split out to keep the update system legible.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn update_meteor_trail(
    trail: &mut MeteorTrailQuery,
    materials: &mut Assets<StandardMaterial>,
    rig: usize,
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
        set_trail_hidden(trail, Some(rig));
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
        set_trail_hidden(trail, Some(rig));
        return;
    }

    // Waver: a bounded lateral lash around the analytic spine (root stiff, tip
    // loose). NOT a re-aim, so it never reintroduces the "pointing around" read.
    let (perp_a, perp_b) = orthonormal_basis(back);
    const WAVER_SALT: u64 = 0xF11C_4E12_0000_0000;
    let phase = (splitmix64(trajectory_seed ^ WAVER_SALT) % 6_283) as f32 / 1_000.0;
    let t = (now / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;

    // Foreshortening flare: widen the first segments when the tail is end-on so
    // it shows a flared skirt hugging the ball instead of a nub. Kept modest,
    // and the root DEEPENS as it flares: at 1.6x the end-on root projected as a
    // big flat disc wrapping the whole silhouette, and at full root intensity
    // that disc tonemapped to a solid tan slab (the "layered sprites" read).
    let f = foreshortening_factor(travel, camera_forward);
    let flare = lerp(1.0, 1.3, f);
    let end_on_deepen = lerp(1.0, 0.78, f);

    let root_intensity = lerp(
        METEOR_TRAIL_ROOT_INTENSITY_ENTRY,
        METEOR_TRAIL_ROOT_INTENSITY_IMPACT,
        descent,
    ) * end_on_deepen;
    let tip_intensity = lerp(
        METEOR_TRAIL_TIP_INTENSITY_ENTRY,
        METEOR_TRAIL_TIP_INTENSITY_IMPACT,
        descent,
    ) * end_on_deepen;

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

    for (mut transform, mut visibility, segment, rig_tag, material) in trail.iter_mut() {
        if rig_tag.0 != rig {
            continue;
        }
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

/// Position, orient, size, and shade one rig's flame tongues for one frame. Each
/// tongue roots on the shell rim, orbits the travel axis slowly (alternating
/// directions so the licks slide past each other instead of rotating as a rigid
/// cage), and flares outward-backward on its own quick seeded pulse, so the
/// fireball's silhouette constantly sprouts and swallows little flame spikes.
/// All motion is deterministic in (trajectory_seed, now), so every client sees
/// the identical dance.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn update_meteor_flame_tongues(
    tongues: &mut MeteorTongueQuery,
    materials: &mut Assets<StandardMaterial>,
    rig: usize,
    render_pos: Vec3,
    travel: Vec3,
    ball_radius: f32,
    descent: f32,
    shimmer: f32,
    trajectory_seed: u64,
    now: f64,
) {
    if travel == Vec3::ZERO || ball_radius <= 0.0 {
        set_tongues_hidden(tongues, Some(rig));
        return;
    }
    let back = -travel;
    let (perp_a, perp_b) = orthonormal_basis(back);
    const TONGUE_SALT: u64 = 0x70D6_0E00_0000_0000;
    let phase = (splitmix64(trajectory_seed ^ TONGUE_SALT) % 6_283) as f32 / 1_000.0;
    let t = (now / f64::from(crate::protocol::SERVER_TICK_RATE_HZ)) as f32;
    let seed_lo = trajectory_seed as u32;
    let n = METEOR_FLAME_TONGUE_COUNT as f32;

    for (mut transform, mut visibility, tongue, rig_tag, material) in tongues.iter_mut() {
        if rig_tag.0 != rig {
            continue;
        }
        *visibility = Visibility::Visible;
        let k = tongue.index;
        let kf = k as f32;
        let h1 = hashed_unit((k as u32).wrapping_mul(0x9E37_79B9) ^ seed_lo);
        let h2 = hashed_unit((k as u32).wrapping_mul(0x85EB_CA6B) ^ seed_lo.rotate_left(13));

        // Slow per-tongue orbit around the travel axis.
        let orbit_dir = if k % 2 == 0 { 1.0 } else { -1.0 };
        let angle = kf / n * std::f32::consts::TAU + phase + t * (0.5 + h1 * 0.9) * orbit_dir;
        let radial = perp_a * angle.cos() + perp_b * angle.sin();

        // The flame beat: one quick pulse drives flare length, sweep-back, and
        // heat together, so each lick genuinely licks.
        let pulse = 0.5 + 0.5 * (t * (5.0 + h2 * 4.0) + phase * 1.3 + kf * 2.1).sin();

        // Slim streaming licks, dialled down toward impact (the close pass was
        // already right without them). Rooted INSIDE the shell and leaning hard
        // toward the tail so they read as fire being dragged off the rock into
        // the trail, never a fat lump ring wrapping the silhouette (the first
        // pass's donut read).
        let prominence = lerp(
            METEOR_TONGUE_PROMINENCE_ENTRY,
            METEOR_TONGUE_PROMINENCE_IMPACT,
            descent,
        );
        let dir = (radial + back * (0.55 + 0.75 * pulse)).normalize_or_zero();
        let length = ball_radius * (0.55 + 0.70 * pulse) * prominence;
        let width = ball_radius * (0.09 + 0.05 * pulse) * prominence;
        transform.translation = render_pos + radial * ball_radius * 0.45 + dir * length * 0.5;
        transform.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
        // Unit-sphere mesh: Y half-extent is half the lick's full length.
        transform.scale = Vec3::new(width, length * 0.5, width);

        if let Some(mut material) = materials.get_mut(&material.0) {
            // Hotter at full flare, deeper at rest, and a touch hotter overall
            // as the ball dives, like the body layers.
            let hue = METEOR_TONGUE_HUE_REST.lerp(METEOR_TONGUE_HUE_FLARE, pulse);
            let color = hue * lerp(0.85, 1.15, descent) * shimmer;
            material.base_color = Color::linear_rgb(color.x, color.y, color.z);
        }
    }
}

/// Throw the mid-flight airburst at the fireball's render position: the hot
/// core flash, a fan of burning fragments, and a one-off radial ember spray.
/// Everything is sized and paced against the RENDERED ball radius (like the
/// shed embers) so the burst reads at the ball's apparent size on the far
/// proxy; `size` additionally thins the fragment/spark COUNTS so a small
/// meteor pops a smaller burst. `remaining_seconds` caps the fragment
/// lifetimes so a burst late in a short `/meteor-here` dive still burns out
/// before the impact. Deterministic in the trajectory seed.
#[expect(clippy::too_many_arguments, reason = "split-out system helper")]
fn spawn_meteor_airburst(
    commands: &mut Commands,
    assets: &MeteorEmberAssets,
    materials: &mut Assets<StandardMaterial>,
    render_pos: Vec3,
    travel: Vec3,
    ball_radius: f32,
    remaining_seconds: f32,
    trajectory_seed: u64,
    size: f32,
) {
    if travel == Vec3::ZERO || ball_radius <= 0.0 {
        return;
    }
    let back = -travel;
    let (perp_a, perp_b) = orthonormal_basis(back);
    let seed_lo = trajectory_seed as u32;
    // Size thins the burst's counts (the geometry already scales off the ball
    // radius); floors keep even a small meteor's pop reading as a real burst.
    let size = size.clamp(0.05, 1.0);
    let fragment_count = ((METEOR_AIRBURST_FRAGMENTS as f32 * size).round() as u32).max(4);
    let spark_count = ((METEOR_AIRBURST_SPARKS as f32 * size).round() as u32).max(10);

    // The core flash: full size from the first frame, fading via its own
    // material instance (freed with the entity, so the per-frame fade never
    // touches a shared handle).
    let flash_material = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(
            METEOR_AIRBURST_FLASH_COLOR.x,
            METEOR_AIRBURST_FLASH_COLOR.y,
            METEOR_AIRBURST_FLASH_COLOR.z,
        ),
        unlit: true,
        fog_enabled: false,
        alpha_mode: AlphaMode::Add,
        ..default()
    });
    let peak_scale = ball_radius * METEOR_AIRBURST_FLASH_PEAK_FRAC;
    commands.spawn((
        Name::new("Meteor Airburst Flash"),
        MeteorAirburstFlash::new(METEOR_AIRBURST_FLASH_SECONDS, METEOR_AIRBURST_FLASH_COLOR),
        Mesh3d(assets.flash_mesh.clone()),
        MeshMaterial3d(flash_material),
        // Born at full size; the pop is carried entirely by the color fade.
        Transform::from_translation(render_pos).with_scale(Vec3::splat(peak_scale)),
        Visibility::Visible,
        NotShadowCaster,
    ));

    // Burning fragments: a fan around the travel axis. Each keeps a slice of the
    // core's onward motion plus a radial kick, so the fragments visibly separate
    // from the ball (which accelerates on ahead) while still falling with it.
    // OPAQUE deep-fire materials (two alternating hues for variety): the first
    // pass reused the bright-additive ember material and the big blobs washed to
    // cream petals under AgX; opaque deep orange holds fire hue at any size, the
    // same lesson as the trail cones.
    let fragment_materials = [METEOR_FRAGMENT_HUE_HOT, METEOR_FRAGMENT_HUE_DEEP].map(|hue| {
        materials.add(StandardMaterial {
            base_color: Color::linear_rgb(hue.x, hue.y, hue.z),
            unlit: true,
            fog_enabled: false,
            ..default()
        })
    });
    for i in 0..fragment_count {
        let s = seed_lo ^ i.wrapping_mul(0x9E37_79B9);
        let r1 = hashed_unit(s);
        let r2 = hashed_unit(s ^ 0x85EB_CA6B);
        let r3 = hashed_unit(s ^ 0xC2B2_AE35);
        let r4 = hashed_unit(s ^ 0x2545_F491);

        let ring = (i as f32 + r1 * 0.8) / fragment_count as f32 * std::f32::consts::TAU;
        let radial = perp_a * ring.cos() + perp_b * ring.sin();
        let dir = (travel * (0.50 + r2 * 0.45) + radial * (0.50 + r3 * 0.50)).normalize_or_zero();
        let speed = ball_radius * (1.3 + r4 * 1.4);
        // Rendered head radius = ember_mesh(0.12) * scale. SMALL: real
        // fragments are chips off the parent rock, and the ball CONVERGES
        // toward its true apparent size after the burst while fragments keep
        // their spawn size, so anything generous ends up reading nearly
        // ball-sized moments later (owner report, twice: the burst fan drew a
        // ring of pale near-ball-sized chunks; the earlier 3.0 cap was still
        // dominated by it). Four fixed variants plus a hard absolute cap so a
        // fragment that arcs down and lands rests as a fist-sized stone.
        let initial_scale =
            crate::app::systems::quantized_chip_scale((ball_radius * 0.35).min(1.2), r2);
        commands.spawn((
            Name::new("Meteor Airburst Fragment"),
            MeteorFragment {
                velocity: dir * speed,
                age: 0.0,
                // Long enough for most fragments to arc all the way down and
                // land; still capped by the time left so a late burst (short
                // /meteor-here dive) cleans its fragments up by the strike.
                lifetime: (3.0 + r3 * 1.6).min((remaining_seconds * 0.9).max(0.4)),
                initial_scale,
                gravity: ball_radius * 0.45,
                ember_cooldown: r1 * METEOR_FRAGMENT_EMBER_INTERVAL,
                seed: s,
                grounded_at: None,
            },
            Mesh3d(assets.ember_mesh.clone()),
            MeshMaterial3d(fragment_materials[i as usize % fragment_materials.len()].clone()),
            Transform::from_translation(render_pos + dir * ball_radius * 0.4)
                .with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }

    // A one-off radial spark spray so the pop itself scatters glitter beyond the
    // solid fragments. Ordinary shed embers; the shared ticker cleans them up.
    for i in 0..spark_count {
        let s = seed_lo
            .rotate_left(11)
            .wrapping_add(i.wrapping_mul(0x27D4_EB2F));
        let r1 = hashed_unit(s);
        let r2 = hashed_unit(s ^ 0x1656_67B1);
        let r3 = hashed_unit(s ^ 0x94D0_49BB);

        let ring = r1 * std::f32::consts::TAU;
        let radial = perp_a * ring.cos() + perp_b * ring.sin();
        let dir = (radial * (0.7 + r2 * 0.5) + back * (r3 - 0.35)).normalize_or_zero();
        // SMALL: the additive spark material only holds its orange as a
        // compact point; at any real size it washes to a cream blob, and the
        // old sizing drew a ring of pale ball-sized blobs around the fireball
        // in daylight (owner report: huge chunks). Glitter, not chunks. Born
        // OUTSIDE the ball so the spray reads as flying off the pop.
        let initial_scale =
            crate::app::systems::quantized_chip_scale((ball_radius * 0.25).min(1.2), r3);
        commands.spawn((
            Name::new("Meteor Ember"),
            MeteorEmber {
                velocity: dir * ball_radius * (0.7 + r2 * 1.0),
                age: 0.0,
                lifetime: 0.6 + r1 * 0.6,
                smoke: false,
            },
            Mesh3d(assets.ember_mesh.clone()),
            MeshMaterial3d(assets.ember_material.clone()),
            Transform::from_translation(render_pos + dir * ball_radius * (0.9 + r2 * 0.4))
                .with_scale(Vec3::splat(initial_scale)),
            Visibility::Visible,
            NotShadowCaster,
        ));
    }
}

/// Shed the ember spark stream (and a sparser dark smoke ribbon) behind one
/// fireball at LOD-gated rates, accumulating fractional emissions across frames.
/// Sparks streak in a tight column hugging the tail (not a scattered cloud); the
/// stream ramps up close (`max(descent, tf)`) so dozens of additive sparks only
/// appear in the last seconds. Sized and spread against the RENDERED ball radius
/// so far particles on the grown proxy read at the ball's apparent size and close
/// ones are physical-scale; the meteor's `size` additionally thins the emission
/// rate so a small rock sputters a sparser stream. `f` is the head-on
/// foreshortening factor: it widens the spark spread so sparks streak across
/// the ball's face when the tail is end-on.
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
    size: f32,
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
    // is confined to the close, final-seconds pass; smoke is far sparser. A small
    // meteor sheds proportionally fewer sparks (eased so it never goes quiet).
    let closeness = descent.max(tf).clamp(0.0, 1.0);
    let spark_rate = lerp(METEOR_EMBER_RATE_FAR, METEOR_EMBER_RATE_CLOSE, closeness)
        * (0.4 + 0.6 * size.clamp(0.0, 1.0));
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
        // ball radius directly. Kept small so each spark stays a distinct
        // high-contrast point that holds its orange, not a fat pale blob. Four
        // fixed size variants with an absolute cap (the ball radius is the
        // rendered proxy size and can reach ~20).
        let initial_scale =
            crate::app::systems::quantized_chip_scale((ball_radius * 0.45).min(2.0), r3);
        let lifetime = 0.7 + r4 * 0.6;

        commands.spawn((
            Name::new("Meteor Ember"),
            MeteorEmber {
                velocity,
                age: 0.0,
                lifetime,
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
        // buoyancy, at a FIXED size for its whole life. The 0.5-radius mesh needs
        // only a modest scale; four fixed variants with an absolute cap.
        let smoke_unit = ball_radius * 0.14;
        let perp = (perp_a * (r2 - 0.5) + perp_b * (r3 - 0.5)) * ball_radius * 0.10;
        let offset = back * ball_radius * (1.0 + r1 * 1.5) - Vec3::Y * ball_radius * 0.15 + perp;
        let velocity = Vec3::Y * smoke_unit * 0.3 + perp * 0.2;
        let initial_scale =
            crate::app::systems::quantized_chip_scale((smoke_unit * 0.85).min(2.2), r3);
        let lifetime = 0.8 + r1 * 0.6;

        commands.spawn((
            Name::new("Meteor Smoke"),
            MeteorEmber {
                velocity,
                age: 0.0,
                lifetime,
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
/// the `smoke` flag: sparks fall under gravity with light drag; smoke drifts up.
/// Both hold their fixed spawn size. World-space, so they linger where shed (the
/// fireball has already moved on). Runs in `ClientSystemSet::Sky`.
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
        // Both kinds hold their spawn scale for their whole (short) life and pop
        // out at the end: no particle ever animates size (owner call; the old
        // grow-over-life smoke and shrink-to-a-point sparks read as particles
        // flickering between sizes). Motion is the only animation.
        if ember.smoke {
            // Smoke: light drift up and away.
            ember.velocity *= 1.0 - (0.4 * dt).min(0.9);
            transform.translation += ember.velocity * dt;
        } else {
            // Spark: gravity + light drag.
            ember.velocity.y -= METEOR_EMBER_GRAVITY * dt;
            ember.velocity *= 1.0 - (0.6 * dt).min(0.9);
            transform.translation += ember.velocity * dt;
        }
    }
}

/// Advance the airburst's transient entities: fragments arc away from the core
/// at a fixed size, sputter ember sparks behind them, and land or expire; the
/// core flash holds its size and fades its color to nothing. Both self-despawn,
/// so an ended event never needs to clean them up. Runs in
/// `ClientSystemSet::Sky` alongside the ember ticker.
pub(crate) fn tick_meteor_airburst_system(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<MeteorEmberAssets>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fragments: Query<
        (Entity, &mut Transform, &mut MeteorFragment),
        Without<MeteorAirburstFlash>,
    >,
    mut flashes: Query<
        (
            Entity,
            &mut MeteorAirburstFlash,
            &MeshMaterial3d<StandardMaterial>,
        ),
        Without<MeteorFragment>,
    >,
) {
    let dt = time.delta_secs().max(0.0);
    if dt == 0.0 {
        return;
    }

    for (entity, mut transform, mut fragment) in &mut fragments {
        fragment.age += dt;
        // A grounded fragment rests where it landed for a beat, then goes.
        if let Some(grounded_at) = fragment.grounded_at {
            if fragment.age - grounded_at >= METEOR_FRAGMENT_REST_SECONDS {
                commands.entity(entity).despawn();
            }
            continue;
        }
        if fragment.age >= fragment.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        fragment.velocity.y -= fragment.gravity * dt;
        fragment.velocity *= 1.0 - (0.35 * dt).min(0.9);
        let step = fragment.velocity * dt;
        transform.translation += step;

        // Fixed size for the whole flight: solid matter holds its scale and
        // only moves under physics (the old burn-up shrink read as particles
        // animating size). A fragment that reaches the ground plane LANDS:
        // it seats on the floor, stops reorienting, and rests.
        let s = fragment.initial_scale;
        let ground_y = s * 0.12;
        if transform.translation.y <= ground_y {
            transform.translation.y = ground_y;
            fragment.grounded_at = Some(fragment.age);
            continue;
        }
        let dir = fragment.velocity.normalize_or_zero();
        if dir != Vec3::ZERO {
            transform.rotation = Quat::from_rotation_arc(Vec3::Y, dir);
        }
        // Elongated along its own flight so it reads as a streaking mini-meteor.
        transform.scale = Vec3::new(s * 0.62, s * 1.7, s * 0.62);

        // Sputter embers behind the head; they linger where shed like the main
        // ball's stream and ride the shared ember ticker. Capped per frame so a
        // long hitch cannot dump a fragment's whole trail at once.
        let Some(assets) = assets.as_ref() else {
            continue;
        };
        fragment.ember_cooldown -= dt;
        let mut shed = 0;
        while fragment.ember_cooldown <= 0.0 && shed < 4 {
            fragment.ember_cooldown += METEOR_FRAGMENT_EMBER_INTERVAL;
            shed += 1;
            fragment.seed = fragment.seed.wrapping_add(0x9E37_79B9);
            let r1 = hashed_unit(fragment.seed);
            let r2 = hashed_unit(fragment.seed ^ 0x85EB_CA6B);
            let r3 = hashed_unit(fragment.seed ^ 0xC2B2_AE35);
            let jitter = Vec3::new(r1 - 0.5, r2 - 0.5, r3 - 0.5) * fragment.initial_scale * 0.8;
            // Kept small: the additive ember material only holds its orange as
            // a compact spark; larger and it whites out. Quantized like every
            // other debris size.
            let ember_scale = crate::app::systems::quantized_chip_scale(
                (fragment.initial_scale * 0.30).max(0.01),
                r2,
            );
            commands.spawn((
                Name::new("Meteor Ember"),
                MeteorEmber {
                    velocity: fragment.velocity * -0.12 + jitter,
                    age: 0.0,
                    lifetime: 0.5 + r1 * 0.5,
                    smoke: false,
                },
                Mesh3d(assets.ember_mesh.clone()),
                MeshMaterial3d(assets.ember_material.clone()),
                Transform::from_translation(transform.translation - dir * s * 0.9)
                    .with_scale(Vec3::splat(ember_scale)),
                Visibility::Visible,
                NotShadowCaster,
            ));
        }
        fragment.ember_cooldown = fragment.ember_cooldown.max(0.0);
    }

    for (entity, mut flash, material) in &mut flashes {
        flash.age += dt;
        if flash.age >= flash.lifetime {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (flash.age / flash.lifetime).clamp(0.0, 1.0);
        // The flash never changes size: the pop is a hard fade of its own
        // additive material instance down to black.
        if let Some(mut material) = materials.get_mut(&material.0) {
            let c = flash.color * (1.0 - t).powf(1.7);
            material.base_color = Color::linear_rgb(c.x, c.y, c.z);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airburst_fires_only_on_a_watched_crossing() {
        let threshold = airburst_threshold(0.0);
        // The normal case: last frame just under the trigger, this frame at or
        // past it (and inside the grace window) fires.
        assert!(airburst_crossing(
            Some(threshold - 0.001),
            threshold + 0.001,
            threshold
        ));
        // First observed frame of the event: no previous descent, never fires,
        // wherever the meteor already is.
        assert!(!airburst_crossing(None, threshold + 0.001, threshold));
        // Already past the trigger on both frames: the crossing happened
        // earlier (and fired then), not now.
        assert!(!airburst_crossing(
            Some(threshold + 0.01),
            threshold + 0.02,
            threshold
        ));
        // Still before the trigger: nothing yet.
        assert!(!airburst_crossing(
            Some(threshold - 0.02),
            threshold - 0.01,
            threshold
        ));
    }

    #[test]
    fn airburst_skips_a_crossing_observed_past_the_grace_window() {
        let threshold = airburst_threshold(0.0);
        // A long menu straddle: the previous observed frame was before the
        // trigger, but by the time we look again the meteor is far past the
        // window. Skip entirely rather than popping a stale burst.
        assert!(!airburst_crossing(
            Some(threshold - 0.01),
            threshold + METEOR_AIRBURST_WINDOW + 0.01,
            threshold
        ));
        // Just inside the window still fires (a brief hitch is fine).
        assert!(airburst_crossing(
            Some(threshold - 0.01),
            threshold + METEOR_AIRBURST_WINDOW * 0.5,
            threshold
        ));
    }

    #[test]
    fn airburst_threshold_anchors_to_the_watched_window() {
        // Watched from the announce (full flight): bursts halfway down.
        assert!((airburst_threshold(0.0) - METEOR_AIRBURST_VISIBLE_FRACTION).abs() < 1e-6);
        // A short /meteor-here dive (8 s of a 45 s arc): first seen at descent
        // ~0.822, so the burst lands halfway through the watched window, a few
        // seconds in, still clearly before impact.
        let first_seen = 1.0 - 8.0 / crate::world::METEOR_FLIGHT_SECONDS;
        let threshold = airburst_threshold(first_seen);
        assert!(
            threshold > first_seen,
            "burst comes after the fireball shows"
        );
        assert!(threshold < 1.0, "and before the impact");
        let expected = first_seen + 0.5 * (1.0 - first_seen);
        assert!((threshold - expected).abs() < 1e-6);
        // Degenerate first-seen values stay sane.
        assert!(airburst_threshold(1.5) <= 1.0);
    }

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
        // Same far point across the drama phase (before the true-size
        // convergence window opens): it should be bigger later.
        let true_pos = Vec3::new(4_000.0, 3_000.0, 0.0);
        let (_, entry_scale, _) = meteor_render_placement(true_pos, camera, 0.0);
        let (_, mid_scale, _) = meteor_render_placement(true_pos, camera, 0.4);
        let (_, late_scale, _) =
            meteor_render_placement(true_pos, camera, METEOR_TRUE_SIZE_CONVERGE_START);
        assert!(
            (entry_scale - 1.0).abs() < 1e-6,
            "at entry the proxy ball is the base size, got {entry_scale}"
        );
        assert!(
            mid_scale > entry_scale && late_scale > mid_scale,
            "the ball swells as it descends: {entry_scale} < {mid_scale} < {late_scale}"
        );
    }

    #[test]
    fn meteor_drama_growth_is_damped_for_far_cameras() {
        let camera = Vec3::ZERO;
        let descent = 0.5;
        // Same descent, two observers: one under the flight path (inside the
        // full-drama distance), one across the map. The far observer sees a
        // much smaller swell: the bearing-down growth is for meteors actually
        // coming at you (owner report: distant meteors read far too large).
        let near_pos = Vec3::new(0.0, 600.0, 0.0);
        let far_pos = Vec3::new(0.0, METEOR_DRAMA_FAR_DISTANCE_M + 500.0, 0.0);
        let (_, near_scale, _) = meteor_render_placement(near_pos, camera, descent);
        let (_, far_scale, _) = meteor_render_placement(far_pos, camera, descent);
        let full_growth = 1.0 + descent * (METEOR_PROXY_GROWTH - 1.0);
        let damped_growth = 1.0 + descent * (METEOR_PROXY_GROWTH - 1.0) * METEOR_DRAMA_FAR_KEEP;
        assert!(
            (near_scale - full_growth).abs() < 1e-4,
            "inside the full-drama band the swell is untouched, got {near_scale}"
        );
        assert!(
            (far_scale - damped_growth).abs() < 1e-4,
            "past the far band only the KEEP fraction of growth survives, got {far_scale}"
        );
        assert!(
            far_scale < near_scale * 0.6,
            "a far crossing reads much smaller"
        );
    }

    #[test]
    fn meteor_far_observer_final_descent_converges_to_true_apparent_size() {
        // A player watching a far-off impact: the meteor never comes near the
        // camera, so the camera-distance blend never engages. The grown drama
        // sprite must still hand over visually: by the end of the descent the
        // rendered scale matches the TRUE angular size of the physical ball at
        // its actual distance, instead of a huge glow that vanishes at impact.
        let camera = Vec3::ZERO;
        let distance = 500.0;
        let true_pos = Vec3::new(400.0, 300.0, 0.0);
        let (_, drama_scale, _) = meteor_render_placement(true_pos, camera, 0.5);
        let (_, end_scale, _) = meteor_render_placement(true_pos, camera, 1.0);
        assert!(drama_scale > 1.0, "mid-flight keeps the drama growth");
        let true_apparent = METEOR_PROXY_DISTANCE / distance;
        assert!(
            (end_scale - true_apparent).abs() < 1e-3,
            "at impact the far observer sees the true angular size: {end_scale} vs {true_apparent}"
        );
        assert!(
            end_scale < drama_scale,
            "the sprite visibly settles down out of the drama phase"
        );
        // And the convergence is progressive, not a snap.
        let (_, converging_scale, _) = meteor_render_placement(
            true_pos,
            camera,
            (METEOR_TRUE_SIZE_CONVERGE_START + METEOR_TRUE_SIZE_CONVERGE_END) * 0.5,
        );
        assert!(converging_scale < drama_scale * METEOR_PROXY_GROWTH);
        assert!(converging_scale > end_scale);
    }

    #[test]
    fn meteor_placement_blends_smoothly_across_the_handoff_band() {
        let camera = Vec3::ZERO;
        // Mid-flight (before the true-size convergence), so the proxy is in
        // its grown drama phase and the band blend is what varies.
        let descent = 0.5;
        let proxy_scale = 1.0 + descent * (METEOR_PROXY_GROWTH - 1.0);
        // Just outside the proxy distance: full proxy (grown), on the sphere.
        let far = Vec3::new(0.0, METEOR_PROXY_DISTANCE + 5.0, 0.0);
        let (far_pos, far_scale, _) = meteor_render_placement(far, camera, descent);
        assert!((far_pos.length() - METEOR_PROXY_DISTANCE).abs() < 1e-2);
        assert!((far_scale - proxy_scale).abs() < 1e-3);

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
