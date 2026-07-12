use std::collections::HashMap;
use std::f32::consts::PI;

use bevy::{
    camera::visibility::RenderLayers, gltf::GltfAssetLabel, light::NotShadowCaster, prelude::*,
};

use crate::{
    app::{
        embedded_asset_path,
        scene::{
            HeldItemVisual, ItemVisualAssets, MainCamera, ToonMaterial, ToonViewmodelMaterial,
            VIEWMODEL_RENDER_LAYER,
        },
        state::{
            GatherInputState, LocalPlayerState, MenuState, RangedDrawState, Screen, ToolSwapState,
        },
    },
    items::{
        HeldGrip, HeldLayerMeshSource, HeldMesh, HeldMeshMaterial, HeldPieceSlot, ItemModel,
        item_definition,
    },
};

use super::swing_poses::{
    ToolSwingPose, bag_idle_pose, bow_draw_pose, bow_release_pose, club_swing_pose, crossbow_pose,
    hatchet_swing_pose, lerp, mace_swing_pose, pickaxe_swing_pose, smoothstep, spear_swing_pose,
    sword_swing_pose, throw_charge_pose, throw_lob_pose,
};

const HELD_ITEM_FORWARD_OFFSET: f32 = 0.62;
const HELD_ITEM_RIGHT_OFFSET: f32 = 0.28;
const HELD_ITEM_DOWN_OFFSET: f32 = 0.24;

/// How long the bow's release flick plays out before the viewmodel settles back
/// to the carry rest, in seconds. Short and snappy: loose is a forward flick, not
/// a swing follow-through.
pub(crate) const BOW_RELEASE_SECONDS: f32 = 0.22;
/// How long the crossbow recoil kick decays over after a shot, in seconds. Punchy
/// and brief so the jolt reads as a hard report, not a wobble.
pub(crate) const CROSSBOW_RECOIL_SECONDS: f32 = 0.18;

/// The live ranged-pose inputs the held-item transform reads for a bow / crossbow,
/// computed from [`crate::app::state::RangedDrawState`] each frame. For a melee /
/// tool item every field is neutral (zero), so the melee swing path is byte
/// unchanged; a bow / crossbow drives its draw / reload / recoil pose off these.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RangedPoseInputs {
    /// Bow draw fraction in `[0, 1]` (0 rest, 1 full draw). Drives the draw pose.
    pub(crate) draw_fraction: f32,
    /// Whether a bow draw is currently being held (drives the draw pose vs the
    /// release/rest pose).
    pub(crate) drawing: bool,
    /// Bow release flick progress in `[0, 1]` (0 just released, 1 settled). Ignored
    /// while `drawing`.
    pub(crate) release_progress: f32,
    /// Crossbow reload fraction in `[0, 1]` (0 just fired, 1 ready). Drives the
    /// crank pose.
    pub(crate) reload_fraction: f32,
    /// Crossbow recoil in `[0, 1]` (1 just fired, 0 settled). Drives the fire kick.
    pub(crate) recoil: f32,
    /// Crossbow aim-down-sights fraction in `[0, 1]` (0 carry, 1 fully aimed).
    /// Slides the stock to the eye line and steadies the idle sway.
    pub(crate) aim: f32,
    /// Thrown-bomb charge wind-up in `[0, 1]` (0 carry rest, 1 fully wound up
    /// at the shoulder), from [`crate::app::state::ThrowChargeState::wind_up`].
    /// Zero for every other item; drives the bomb's hold-to-charge pose.
    pub(crate) throw_wind_up: f32,
    /// Seconds elapsed, seeding the draw tremble noise.
    pub(crate) time_seconds: f32,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_held_item_visual_system(
    mut commands: Commands,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    visuals: Res<HeldItemVisuals>,
    gather_input: Res<GatherInputState>,
    ranged: Res<RangedDrawState>,
    throw_charge: Res<crate::app::state::ThrowChargeState>,
    swap_state: Res<ToolSwapState>,
    time: Res<Time>,
    camera: Query<Entity, With<MainCamera>>,
    held: Query<(Entity, &HeldItemVisual)>,
) {
    // Dead players don't render a held item. The corpse drops its
    // loot into a bag at the death position; the camera fades to
    // black while the death splash is up, and a visible weapon on
    // screen during that fade reads as a UI bug.
    let local_dead = matches!(
        local_player.lifecycle,
        Some(crate::server::PlayerLifecycle::Dead { .. })
    );
    // Panel overlays (inventory, crafting, furnace, loot bag, workbench, map)
    // also hide the held item: the viewmodel camera composites after the UI
    // pass, so a visible tool would draw on top of the open panel. The
    // world-entry loading splash hides it for the same reason: the viewmodel
    // camera renders after egui, so the held tool would float on top of the
    // opaque loading overlay.
    let active_item = (menu.screen == Screen::InGame
        && !menu.pause_open
        && !menu.panel_overlay_open()
        && !menu.world_entry_splash_active()
        && !local_dead)
        .then(|| {
            local_player
                .private
                .as_ref()
                .and_then(|private| private.inventory.active_actionbar_stack())
                .and_then(|stack| {
                    item_definition(&stack.item_id)
                        .map(|definition| (stack.item_id.clone(), definition))
                })
        })
        .flatten();

    let Some((item_id, definition)) = active_item.filter(|(_, definition)| definition.equipable)
    else {
        for (entity, _) in &held {
            commands.entity(entity).despawn();
        }
        return;
    };

    let Ok(camera_entity) = camera.single() else {
        return;
    };
    // Live ranged-pose inputs off the draw state (neutral for a melee / tool
    // item, so the swing path stays byte-identical). Drives the bow draw / release
    // and the crossbow recoil / reload crank on the first-person viewmodel.
    let ranged_pose = RangedPoseInputs {
        draw_fraction: ranged.draw_fraction(),
        drawing: ranged.is_drawing(),
        release_progress: ranged.release_progress(BOW_RELEASE_SECONDS),
        reload_fraction: ranged.reload_fraction(),
        recoil: ranged.recoil(CROSSBOW_RECOIL_SECONDS),
        aim: ranged.aim_fraction(),
        throw_wind_up: throw_charge.wind_up(),
        time_seconds: time.elapsed_secs(),
    };
    // The whole-item swing/carry transform, shared by every layer. Each layer then
    // composes its own per-piece local transform on top (identity for static
    // pieces, a flex / slide for the bow limbs / string and the crossbow string),
    // so the animatable ranged weapons bend without any extra ECS machinery.
    let whole_item = apply_idle_sway(
        held_item_local_transform(
            definition.model,
            definition.held_mesh,
            gather_input.swing_fraction(),
            swap_state.fraction(),
            ranged_pose,
        ),
        time.elapsed_secs(),
        // A held ADS steadies the hands: the sway damps toward (not to) zero
        // at full aim so the sight picture calms without reading frozen.
        1.0 - 0.7 * ranged_pose.aim.clamp(0.0, 1.0),
    );
    // First-person-only shrink for meshes authored at world scale (the placed
    // charges double as their own held models): applied on the whole-item
    // transform so per-piece offsets scale with the mesh. Third-person rigs
    // and projectile visuals use the same layers UNSCALED (world props).
    let whole_item = whole_item.with_scale(Vec3::splat(definition.held_mesh.viewmodel_scale()));
    // Compose the whole-item transform with a layer's per-piece transform. The
    // layer entity is a camera-child, so its Transform is the full camera-local
    // transform; the piece transform is applied in the item's own frame (right
    // multiply), matching how a static piece (identity) leaves the whole-item
    // transform untouched.
    let layer_transform = |slot: HeldPieceSlot| -> Transform {
        let piece = held_piece_local_transform(definition.model, slot, ranged_pose);
        whole_item * piece
    };
    // The held item renders as one or more material layers sharing the whole-item
    // swing transform: most items are a single layer, iron tools two (a matte
    // handle body + a shiny iron head), the bow five (grip, two limbs, two string
    // legs). Each layer is its own camera-child entity tagged with the active
    // `item_id` and its rig slot.
    //
    // Steady state (layers already match the active item): just drive the composed
    // per-layer transform onto each layer off its stored slot, the cheap per-frame
    // path. The hierarchy/mesh/material are only (re)built when the held item
    // changes, so we don't retrigger change-detection or hierarchy fix-ups every
    // frame.
    let held_entities: Vec<(Entity, HeldPieceSlot)> =
        held.iter().map(|(entity, v)| (entity, v.slot)).collect();
    let layers_match_item =
        !held_entities.is_empty() && held.iter().all(|(_, visual)| visual.item_id == item_id);

    if layers_match_item {
        for (entity, slot) in held_entities {
            commands.entity(entity).insert(layer_transform(slot));
        }
        return;
    }

    // Held item changed: tear down the old layers and rebuild for the new one.
    for (entity, _) in held_entities {
        commands.entity(entity).despawn();
    }
    for layer in held_item_layers(&visuals, definition.held_mesh, true) {
        let mut spawned = commands.spawn((
            Name::new("Held Item"),
            HeldItemVisual {
                item_id: item_id.clone(),
                slot: layer.slot,
            },
            ChildOf(camera_entity),
            Mesh3d(layer.mesh),
            layer_transform(layer.slot),
            Visibility::Visible,
            // Draw the in-hand item only through the dedicated `ViewmodelCamera`
            // (this layer), never the world camera. That camera's separate cleared
            // depth buffer is what stops the tool clipping into nearby geometry.
            // The third-person tool on remote players is spawned elsewhere and
            // stays on the world layer, so peers still see it lit by the scene.
            RenderLayers::layer(VIEWMODEL_RENDER_LAYER),
            // Held items sit right in front of the camera; their shadow would
            // slash across the floor like a phantom player and dominate the
            // frame. Skip the shadow pass.
            NotShadowCaster,
        ));
        insert_held_layer_material(&mut spawned, layer.material);
    }
}

/// A held-item layer's material. Each is a different asset type, so
/// `MeshMaterial3d<T>` is a different component: this enum lets `held_item_layers`
/// mix them and `insert_held_layer_material` attaches the right one.
/// - `Standard`: the bag silhouette.
/// - `Toon`: world-lit cel tool, used for the THIRD-PERSON tool on a remote
///   player's hand (lit by the scene like every other world prop).
/// - `ToonViewmodel`: camera-relative cel tool, used for the FIRST-PERSON in-hand
///   tool so its bands stay stable as the camera turns.
#[derive(Clone)]
pub(crate) enum HeldLayerMaterial {
    Standard(Handle<StandardMaterial>),
    Toon(Handle<ToonMaterial>),
    ToonViewmodel(Handle<ToonViewmodelMaterial>),
}

/// Attach the correct `MeshMaterial3d<T>` component for a held-item layer. Shared
/// by the first-person viewmodel and the third-person rig so both pick the same
/// material kind per layer.
pub(crate) fn insert_held_layer_material(layer: &mut EntityCommands, material: HeldLayerMaterial) {
    match material {
        HeldLayerMaterial::Standard(handle) => {
            layer.insert(MeshMaterial3d(handle));
        }
        HeldLayerMaterial::Toon(handle) => {
            layer.insert(MeshMaterial3d(handle));
        }
        HeldLayerMaterial::ToonViewmodel(handle) => {
            layer.insert(MeshMaterial3d(handle));
        }
    }
}

/// Precomputed in-hand layer stacks for every [`HeldMesh`], both first-person
/// (viewmodel) and third-person (world) variants. Built once from the declarative
/// [`HeldMesh::visual`] table in `setup_scene` (see [`build_held_item_visuals`]),
/// then read by a plain map lookup in [`held_item_layers`], so adding a held item
/// is one table row plus its glb, not per-item fields + match arms across three
/// files.
#[derive(Resource)]
pub(crate) struct HeldItemVisuals {
    /// FIRST-PERSON in-hand layers (camera-relative `ToonViewmodelMaterial` for
    /// tool layers, so the cel bands don't swim as the camera turns).
    viewmodel: HashMap<HeldMesh, Vec<HeldItemLayer>>,
    /// THIRD-PERSON layers on a remote player's hand (world-lit `ToonMaterial`,
    /// so the tool is lit by the scene like every other world prop).
    world: HashMap<HeldMesh, Vec<HeldItemLayer>>,
}

impl HeldItemVisuals {
    fn variant(&self, viewmodel: bool) -> &HashMap<HeldMesh, Vec<HeldItemLayer>> {
        if viewmodel {
            &self.viewmodel
        } else {
            &self.world
        }
    }
}

/// Resolve a [`HeldMeshMaterial`] family to its concrete layer material, picking
/// the world-lit or camera-relative variant. Keeping this the single family ->
/// handle mapping means the in-flight tools-PBR rework can flip a family to a
/// `StandardMaterial` here without touching the per-item table.
fn resolve_material(
    family: HeldMeshMaterial,
    assets: &ItemVisualAssets,
    viewmodel: bool,
) -> HeldLayerMaterial {
    use HeldLayerMaterial::{Standard, Toon, ToonViewmodel};
    match family {
        // The bag silhouette is a flat `StandardMaterial` in both views.
        HeldMeshMaterial::BagStandard => Standard(assets.held_bag_material.clone()),
        HeldMeshMaterial::Wood if viewmodel => ToonViewmodel(assets.tool_wood_vm_material.clone()),
        HeldMeshMaterial::Wood => Toon(assets.tool_wood_material.clone()),
        HeldMeshMaterial::Stone if viewmodel => {
            ToonViewmodel(assets.tool_stone_vm_material.clone())
        }
        HeldMeshMaterial::Stone => Toon(assets.tool_stone_material.clone()),
        HeldMeshMaterial::Iron if viewmodel => ToonViewmodel(assets.tool_iron_vm_material.clone()),
        HeldMeshMaterial::Iron => Toon(assets.tool_iron_material.clone()),
        HeldMeshMaterial::Parchment if viewmodel => {
            ToonViewmodel(assets.tool_parchment_vm_material.clone())
        }
        HeldMeshMaterial::Parchment => Toon(assets.tool_parchment_material.clone()),
        // The explosive families bind their own dedicated cel materials. Cloth
        // and Leather share the woven-cloth tile (each glb's COLOR_0 gives the
        // fabric its light colour vs the tan leather strap).
        HeldMeshMaterial::Cloth if viewmodel => {
            ToonViewmodel(assets.tool_cloth_vm_material.clone())
        }
        HeldMeshMaterial::Cloth => Toon(assets.tool_cloth_material.clone()),
        HeldMeshMaterial::Leather if viewmodel => {
            ToonViewmodel(assets.tool_cloth_vm_material.clone())
        }
        HeldMeshMaterial::Leather => Toon(assets.tool_cloth_material.clone()),
        HeldMeshMaterial::Cord if viewmodel => ToonViewmodel(assets.tool_cord_vm_material.clone()),
        HeldMeshMaterial::Cord => Toon(assets.tool_cord_material.clone()),
    }
}

/// Fold the declarative [`HeldMesh::visual`] table into the [`HeldItemVisuals`]
/// lookup resource, resolving each layer's mesh source (a glb primitive loaded
/// via `asset_server`, or the shared procedural bag mesh) and material family (to
/// the shared handles on `assets`) for both the first-person and third-person
/// variants. Called once from `setup_scene`.
pub(crate) fn build_held_item_visuals(
    asset_server: &AssetServer,
    assets: &ItemVisualAssets,
) -> HeldItemVisuals {
    let prim_mesh = |glb: &str, primitive: usize| -> Handle<Mesh> {
        asset_server.load(
            GltfAssetLabel::Primitive { mesh: 0, primitive }.from_asset(embedded_asset_path(glb)),
        )
    };
    let mesh_handle = |source: HeldLayerMeshSource| -> Handle<Mesh> {
        match source {
            HeldLayerMeshSource::ProceduralBag => assets.held_bag_mesh.clone(),
            HeldLayerMeshSource::GlbPrimitive { glb, primitive } => prim_mesh(glb, primitive),
        }
    };
    // Load each layer's glb primitive at most once and share the handle between
    // the two variants (they differ only in material, never mesh), so a two-view
    // build doesn't load the same primitive twice.
    let mut viewmodel = HashMap::new();
    let mut world = HashMap::new();
    for &held_mesh in HeldMesh::ALL {
        let visual = held_mesh.visual();
        let mut vm_layers = Vec::new();
        let mut world_layers = Vec::new();
        for layer in visual.layers() {
            let mesh = mesh_handle(layer.mesh);
            vm_layers.push(HeldItemLayer {
                mesh: mesh.clone(),
                material: resolve_material(layer.material, assets, true),
                slot: layer.slot,
            });
            world_layers.push(HeldItemLayer {
                mesh,
                material: resolve_material(layer.material, assets, false),
                slot: layer.slot,
            });
        }
        viewmodel.insert(held_mesh, vm_layers);
        world.insert(held_mesh, world_layers);
    }
    HeldItemVisuals { viewmodel, world }
}

/// Mesh + material layers that make up the in-hand visual for `held_mesh`.
/// One entry for single-material items; two for the authored tool glbs, whose
/// matte haft body and worked head need different materials (Bevy binds one
/// material per mesh). Layers share the mesh-local frame so they overlay exactly
/// under the same swing transform. A plain map lookup into the precomputed
/// [`HeldItemVisuals`] resource.
///
/// Shared with the third-person rig (`app::systems::players`), which attaches
/// the same layers to a remote player's hand anchor so peers see what's held.
///
/// `viewmodel` picks the tool material set: `true` for the FIRST-PERSON in-hand
/// item (camera-relative `ToonViewmodelMaterial`, stable bands), `false` for the
/// THIRD-PERSON tool on a remote player's hand (world-lit `ToonMaterial`).
pub(crate) fn held_item_layers(
    visuals: &HeldItemVisuals,
    held_mesh: HeldMesh,
    viewmodel: bool,
) -> Vec<HeldItemLayer> {
    visuals
        .variant(viewmodel)
        .get(&held_mesh)
        .cloned()
        .unwrap_or_default()
}

/// One resolved in-hand layer: its mesh, its material, and its rig slot. The slot
/// tags the spawned layer entity so the per-frame update can compose a per-piece
/// local transform (bow limbs / string, crossbow string) with the whole-item
/// swing transform. Static slots get an identity local transform, so single-layer
/// items are unchanged.
#[derive(Clone)]
pub(crate) struct HeldItemLayer {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: HeldLayerMaterial,
    pub(crate) slot: HeldPieceSlot,
}

/// Local transform that seats a held tool in a remote player's hand anchor
/// (third-person). The hand anchor shares the forearm's frame (it hangs -Y from
/// the elbow, -Z forward), so the tool is rotated to run the haft forward out of
/// the fist with the head leading. Scale stays at the authored real-world size.
/// Tuned to read in the hand; the swing arc comes from rotating the arm, not the
/// tool, so this stays a fixed grip.
/// Right upper-arm rotation for the tool-carry pose (relative to the torso),
/// shared with the rig animator so the grip below stays in sync with the arm.
/// Brings the upper arm forward and slightly tucked toward the body.
pub(crate) fn carry_upper_arm_rotation() -> Quat {
    Quat::from_rotation_x(0.6) * Quat::from_rotation_z(-0.12)
}

/// Right forearm rotation for the carry pose (relative to the upper arm): a bent
/// elbow that brings the hand forward to about waist height in front of the body.
pub(crate) fn carry_forearm_rotation() -> Quat {
    Quat::from_rotation_x(1.0)
}

/// Accumulated rotation of the hand anchor (upper arm × forearm) in the carry
/// pose. The grip is derived from its inverse so the held tool ends at a chosen
/// orientation *relative to the player* no matter how the carry joints are split.
fn carry_anchor_rotation() -> Quat {
    carry_upper_arm_rotation() * carry_forearm_rotation()
}

pub(crate) fn held_item_hand_transform(held_mesh: HeldMesh) -> Transform {
    // The tool glbs (and the procedural hammer) are authored with the haft along
    // +Y, the butt near y = -0.51, and the *origin up near the head* (~65% up
    // the handle). With the posed carry arm, the hand is bent up in front of the
    // body, so we DERIVE the grip from the carry pose: pick the tool's desired
    // orientation in the player's frame (`desired`), then `grip = carry_anchor⁻¹
    // · desired` cancels the arm rotation so the tool lands exactly at `desired`
    // however the carry joints are tuned. `grip_y` is where down the handle the
    // hand grips (negative = toward the butt); we then translate that point onto
    // the hand anchor so the tool isn't held by its head.
    //
    // `desired`: haft tilted ~23° forward of vertical (head up-forward), with the
    // bladed tools' heads (authored spanning X) yawed to face forward (-Z).
    let tilt = Quat::from_rotation_x(-0.4);
    let yaw = Quat::from_rotation_y(PI * 0.5);
    // The grip is keyed on the mesh's carry archetype (data on the item
    // registry), not on the mesh variant itself, so a new HeldMesh that reuses
    // an existing carry shape needs no change here.
    let (desired, grip_y) = match held_mesh.grip() {
        // Mallet head strikes along its local Z, so no yaw, just the tilt. The
        // short one-handed grip sits below the head (handle ~0.01-0.19), so the
        // hand grips around the middle of the haft.
        HeldGrip::Mallet => (tilt, 0.10),
        // Long-hafted tools/weapons carry their head in the X plane, so the
        // quarter-turn yaw faces it forward; gripped low toward the butt.
        HeldGrip::LongHafted => (tilt * yaw, -0.16),
        // Bag / building-plan silhouettes have no handle; just sit upright.
        HeldGrip::Silhouette => (Quat::IDENTITY, 0.0),
    };
    let rotation = carry_anchor_rotation().inverse() * desired;
    // Place the grip point at the hand anchor, plus a small seat into the palm.
    let grip = rotation * Vec3::new(0.0, grip_y, 0.0);
    let translation = -grip + Vec3::new(0.0, -0.01, -0.02);
    Transform::from_translation(translation).with_rotation(rotation)
}

/// Very subtle passive sway layered on top of the rest/swing pose so the
/// viewmodel breathes instead of sitting perfectly still. Two slow sine waves
/// at incommensurate frequencies (a gentle Lissajous) avoid a robotic
/// back-and-forth, and the amplitudes are deliberately tiny, a few millimetres
/// of drift and well under a degree of tilt, so it reads as "alive" without
/// ever competing with the swing or the aim. Always applied; during a swing
/// the much larger swing motion swamps it. `steadiness` scales the amplitudes
/// (`1` = full sway, lower = calmer): the crossbow ADS damps it so the aimed
/// sight picture settles.
fn apply_idle_sway(transform: Transform, t: f32, steadiness: f32) -> Transform {
    let s = steadiness.clamp(0.0, 1.0);
    let drift = Vec3::new(
        (t * 0.9).sin() * 0.0055 * s,
        (t * 1.3 + 0.7).sin() * 0.0040 * s,
        0.0,
    );
    let tilt = Quat::from_euler(
        EulerRot::XYZ,
        (t * 1.1).sin() * 0.0090 * s,
        (t * 0.8 + 1.3).sin() * 0.0110 * s,
        (t * 1.0 + 0.4).sin() * 0.0070 * s,
    );
    Transform {
        translation: transform.translation + drift,
        rotation: tilt * transform.rotation,
        scale: transform.scale,
    }
}

/// Peak limb flex angle at full draw, radians (authored `BOW_LIMB_FLEX`). The
/// upper limb rotates `+flex*draw` about the flex axis, the lower `-flex*draw`,
/// so both tips curl BACK toward the archer (in-game +X, the string side) as
/// the draw ramps, the way a real stave bows under the string's pull. The
/// original signs bent the tips the other way, toward the target, which read
/// as the bow flexing backwards (owner report). Tuned down from the earlier
/// 0.62 (~35 deg per tip), which over-bent the stave into an
/// about-to-snap read (owner report); ~20 deg per tip is a clearly loaded
/// but healthy bend.
const BOW_LIMB_FLEX: f32 = 0.35;

/// The bow's model-local RIG geometry, expressed in the glb's frame (which is
/// already in-game coordinates, since the glb is post-export). All pivots /
/// anchors come straight from the authoring rig spec mapped through the export
/// (authoring (x,y,z) -> in-game (x, z, -y)), so authoring Z (limb axis) is
/// in-game Y and authoring Y (flex axis) is in-game -Z.
mod bow_rig {
    use bevy::prelude::*;

    /// Convert an authoring-space point `(x, y, z)` to the glb's in-game frame.
    pub(super) const fn from_authoring(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, z, -y)
    }

    /// The flex axis: authoring +Y maps to in-game -Z. Rotating "about authoring
    /// +Y by θ" is a rotation about this axis by θ.
    pub(super) fn flex_axis() -> Vec3 {
        Vec3::NEG_Z
    }
}

/// Local transform for one animatable held-item PIECE, composed on top of the
/// whole-item swing/carry transform. Returns [`Transform::IDENTITY`] for every
/// static piece (all melee / tool layers, the bow grip, the crossbow stock /
/// iron), so those items render exactly as before. Only the bow limbs / string
/// legs and the crossbow string carry a driven transform:
///
/// - Bow limbs flex about their authored pivots as the draw ramps (upper toward
///   the target, lower mirrored), and the string legs rotate about their limb
///   tips so their shared free (nock) end tracks the drawn nock point, forming a
///   deep V toward the archer at full draw.
/// - The crossbow string slides forward on release / back on the reload crank
///   (its nut translating along the down-range axis), each leg rotating about its
///   limb tip to track the nut.
fn held_piece_local_transform(
    model: ItemModel,
    slot: HeldPieceSlot,
    ranged: RangedPoseInputs,
) -> Transform {
    match (model, slot) {
        (ItemModel::Bow, HeldPieceSlot::BowLimbUpper) => bow_limb_transform(bow_draw(ranged), true),
        (ItemModel::Bow, HeldPieceSlot::BowLimbLower) => {
            bow_limb_transform(bow_draw(ranged), false)
        }
        (ItemModel::Bow, HeldPieceSlot::BowStringUpper) => {
            bow_string_transform(bow_draw(ranged), true)
        }
        (ItemModel::Bow, HeldPieceSlot::BowStringLower) => {
            bow_string_transform(bow_draw(ranged), false)
        }
        (ItemModel::Bow, HeldPieceSlot::BowArrow) => bow_arrow_transform(ranged),
        (ItemModel::Crossbow, HeldPieceSlot::CrossbowString) => {
            crossbow_string_transform(crossbow_cock(ranged))
        }
        (ItemModel::Crossbow, HeldPieceSlot::CrossbowBolt) => {
            crossbow_bolt_transform(crossbow_cock(ranged))
        }
        // Every static piece (and any slot that doesn't match its model) is the
        // whole-item transform alone.
        _ => Transform::IDENTITY,
    }
}

/// The bow's effective draw fraction for the rig, `0` at rest, `1` at full draw.
/// While drawing it is the live draw fraction; just after loose the release flick
/// relaxes the limbs back to rest, so the rig follows `1 - release_progress` so the
/// limbs spring forward as the string snaps off the cheek.
fn bow_draw(ranged: RangedPoseInputs) -> f32 {
    if ranged.drawing {
        ranged.draw_fraction.clamp(0.0, 1.0)
    } else {
        // Right after loose (release_progress 0) the limbs are still bent from the
        // shot and spring forward as the flick settles (progress -> 1 => draw -> 0).
        (1.0 - ranged.release_progress).clamp(0.0, 1.0)
    }
}

/// The bow's NOCK point in the glb (in-game) frame at a given draw. At rest the
/// nock sits at the authored (0.16, 0, 0). At full draw it pulls straight back
/// toward the archer along the bow's own archer axis (in-game +X) with a small
/// drop toward the anchor (-Y), staying entirely in the bow's string plane. See
/// [`BOW_VIEWMODEL_FULL_NOCK`]. This is the client VIEWMODEL geometry only; the
/// server's shot direction is unaffected.
fn bow_nock_point(draw: f32) -> Vec3 {
    let d = draw.clamp(0.0, 1.0);
    // Rest nock is the authored (0.16, 0, 0); lerp to the full-draw nock, pulled
    // straight back toward the archer (+X) in the glb frame.
    let rest = bow_rig::from_authoring(0.16, 0.0, 0.0);
    rest.lerp(BOW_VIEWMODEL_FULL_NOCK, d)
}

/// Full-draw nock in the glb (in-game) frame for the FIRST-PERSON viewmodel.
///
/// The pull is a straight draw ALONG THE BOW'S OWN ARCHER AXIS (+X, the string
/// side) with a small drop toward the anchor (-Y): the whole displacement stays
/// in the bow's string plane, so wherever the draw pose yaws / rolls / trembles
/// the rig, the string stays visibly welded to the stave and pulls back with it.
/// The earlier value added a lateral (+Z) out-of-plane component to fake a
/// camera-facing V; that made the string appear to pull toward the PLAYER
/// independently of the bow's rotation (owner report: the line wasn't anchored
/// to the bow). The whole-item draw yaw turns the string plane slightly toward
/// the camera so the drawn V still reads from the side. Client VIEWMODEL
/// geometry only; the server's shot direction is unaffected.
const BOW_VIEWMODEL_FULL_NOCK: Vec3 = Vec3::new(0.40, -0.04, 0.0);

/// The rotation one bow limb applies at a given draw: about the flex axis, by
/// `+flex*draw` (upper) or `-flex*draw` (lower), which curls each tip back
/// toward the archer (+X, the string side). Shared by [`bow_limb_transform`]
/// (the whole-limb pivot transform) and [`flexed_limb_tip`] (where the tip lands
/// after the flex) so the string tracks exactly where the limb bent to.
fn bow_limb_flex_rotation(draw: f32, upper: bool) -> Quat {
    let angle = if upper {
        BOW_LIMB_FLEX * draw
    } else {
        -BOW_LIMB_FLEX * draw
    };
    Quat::from_axis_angle(bow_rig::flex_axis(), angle)
}

/// The limb pivot in the glb frame (authoring (-0.1079, 0, +/-0.085)): upper +z,
/// lower -z along the limb axis. The tip flexes about this as the draw ramps.
fn bow_limb_pivot(upper: bool) -> Vec3 {
    bow_rig::from_authoring(-0.1079, 0.0, if upper { 0.085 } else { -0.085 })
}

/// The transform for one bow limb: a rotation about its authored pivot, by
/// `-flex*draw` (upper) or `+flex*draw` (lower) about the flex axis.
fn bow_limb_transform(draw: f32, upper: bool) -> Transform {
    pivot_rotation(bow_limb_pivot(upper), bow_limb_flex_rotation(draw, upper))
}

/// Where a limb TIP lands after the draw's flex: the authored rest tip rotated
/// about the limb pivot by the same flex the limb piece applies. Anchoring each
/// string leg here (rather than the static rest tip) keeps the string connected to
/// the bent limb, so the limb flex actually reads: as the stave bows in under load
/// the string ends ride inward with the tips instead of floating off them.
fn flexed_limb_tip(draw: f32, upper: bool) -> Vec3 {
    let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
    let pivot = bow_limb_pivot(upper);
    pivot + bow_limb_flex_rotation(draw, upper) * (rest_tip - pivot)
}

/// The transform for one bow string leg. The leg is authored running from its LIMB
/// TIP (its pinned/anchored end) to the shared nock (its free end). At full draw we
/// want it to run from the FLEXED tip (where the limb bent to under load) to the
/// DRAWN nock, so the string stays welded to the bent limb at one end and to the
/// pulled arrow nock at the other, forming a deep V toward the archer.
///
/// The rest leg runs straight from the tip to the rest nock along model -Y, so its
/// LENGTH axis is Y and its slim square cross-section lives in the X-Z plane. We
/// stretch it by the length ratio along Y ONLY (a non-uniform scale, cross-section
/// left at 1.0) so the cord lengthens without fattening into a plank, rotate the
/// stretched leg from its rest direction onto the drawn direction, then translate
/// it so its anchored end lands on the flexed tip and its free end reaches the
/// drawn nock exactly.
fn bow_string_transform(draw: f32, upper: bool) -> Transform {
    // Authored rest geometry: the leg runs from the rest tip (pinned) to the rest
    // nock (free). Its length axis is the rest tip -> rest nock direction.
    let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
    let rest_nock = bow_nock_point(0.0);
    // Target geometry: from the flexed tip to the drawn nock, so the string tracks
    // the bent limb (the flex reads) and the pulled arrow nock (the V deepens).
    let flexed_tip = flexed_limb_tip(draw, upper);
    let drawn_nock = bow_nock_point(draw);

    let rest_vec = rest_nock - rest_tip;
    let drawn_vec = drawn_nock - flexed_tip;
    let rest_len = rest_vec.length();
    // Stretch along the rest leg's own length axis only, so the cord stays slim at
    // any draw depth (a uniform scale would fatten the cross-section into a plank).
    let stretch = if rest_len > 1e-6 {
        drawn_vec.length() / rest_len
    } else {
        1.0
    };
    let from = rest_vec.normalize_or_zero();
    let to = drawn_vec.normalize_or_zero();
    let rotation = Quat::from_rotation_arc(from, to);
    let scale = Vec3::new(1.0, stretch, 1.0);
    // Compose: p |-> flexed_tip + rotation * (scale * (p - rest_tip)). This maps the
    // authored rest_tip onto the flexed tip and, since scale*|rest_vec| = |drawn_vec|
    // and rotation carries `from` onto `to`, the authored rest_nock onto the drawn
    // nock. The translation term is what pivot_transform emits, offset so the pivot
    // (rest_tip) relocates to flexed_tip rather than staying fixed.
    Transform {
        translation: flexed_tip - rotation * (scale * rest_tip),
        rotation,
        scale,
    }
}

/// The nocked arrow's transform: a rigid translate that keeps its authored nock
/// end welded to the string nock, so the ready arrow slides back with the draw
/// and its exposed tip becomes the full-draw aim reference. Right after loose
/// the piece collapses to the nock point (the real arrow is flying down-range)
/// and grows back in over the tail of the release flick, reading as the archer
/// nocking the next arrow.
fn bow_arrow_transform(ranged: RangedPoseInputs) -> Transform {
    let rest_nock = bow_nock_point(0.0);
    let nock = bow_nock_point(bow_draw(ranged));
    let regrow = if ranged.drawing {
        1.0
    } else {
        // Gone for the first 60% of the release window, then a quick grow-in.
        ((ranged.release_progress - 0.6) / 0.4).clamp(0.0, 1.0)
    };
    let scale = Vec3::splat(regrow);
    // p |-> nock + scale*(p - rest_nock): the authored nock end lands exactly on
    // the drawn nock at any draw, and the collapse shrinks about the nock rather
    // than the bow grip.
    Transform {
        translation: nock - scale * rest_nock,
        rotation: Quat::IDENTITY,
        scale,
    }
}

/// The crossbow string's effective cock fraction, `1` = cocked (pulled back, the
/// ready state), `0` = released (forward). At ready it sits fully cocked. A fresh
/// shot snaps it forward (recoil 1 => cock 0); the reload crank draws it back
/// (reload_fraction 0 -> 1 => cock 0 -> 1). Recoil dominates the instant of the
/// shot, then the reload owns the crank back to cocked.
fn crossbow_cock(ranged: RangedPoseInputs) -> f32 {
    // Just fired: the recoil term forces the string forward (cock toward 0). As the
    // reload cranks, cock rises with the reload fraction back to 1 (cocked/ready).
    // When idle (no recoil, no reload), the crossbow sits ready and cocked.
    let released_by_recoil = ranged.recoil.clamp(0.0, 1.0);
    let cocked_by_reload = ranged.reload_fraction.clamp(0.0, 1.0);
    // If a reload is in progress, the string tracks the crank (0 just fired -> 1
    // ready). Otherwise it is at rest cocked, briefly knocked forward by recoil.
    if cocked_by_reload > 0.0 {
        cocked_by_reload
    } else {
        1.0 - released_by_recoil
    }
}

/// The crossbow string's transform. The nut translates along the down-range axis
/// (authoring Z -> in-game Y): cocked (cock 1) it sits back near the trigger
/// (z_nut 0.115), released (cock 0) it sits forward at the prod (z_nut 0.260). The
/// whole string primitive is modelled at the cocked rest, so we translate it by
/// the delta from cocked to the current nut position along the down-range axis.
fn crossbow_string_transform(cock: f32) -> Transform {
    // z_nut = lerp(0.260 released, 0.115 cocked). The string glb is authored at
    // the cocked nut (z 0.115), so the offset is (z_nut(cock) - 0.115) along
    // authoring Z, which maps to in-game +Y.
    let z_nut = lerp(0.260, 0.115, cock.clamp(0.0, 1.0));
    let delta_authoring_z = z_nut - 0.115;
    // Authoring +Z -> in-game +Y.
    let translation = Vec3::Y * delta_authoring_z;
    Transform::from_translation(translation)
}

/// The loaded bolt's transform: glued to the string (the same nut-following
/// slide), visible only while the crossbow is at or near cocked. On fire the
/// cock snaps to 0 and the bolt collapses (the real projectile is flying); as
/// the reload crank finishes (the last ~15% of the cock) it scales back in,
/// reading as the next bolt being seated against the latched string.
fn crossbow_bolt_transform(cock: f32) -> Transform {
    let seated = ((cock.clamp(0.0, 1.0) - 0.85) / 0.15).clamp(0.0, 1.0);
    let mut transform = crossbow_string_transform(cock);
    transform.scale = Vec3::splat(seated);
    transform
}

/// A rotation `rotation` about a pivot point `pivot` (both in the piece's local
/// frame): translate the pivot to the origin, rotate, translate back.
fn pivot_rotation(pivot: Vec3, rotation: Quat) -> Transform {
    pivot_transform(pivot, rotation, Vec3::ONE)
}

/// A per-axis scale (in the piece's local frame) then rotation about a pivot point:
/// translate the pivot to the origin, scale, rotate, translate back. Bevy applies
/// scale, then rotation, then translation, so applied to a point `p` this yields
/// `pivot + rotation*(scale*(p - pivot))`, and the translation that reproduces the
/// pivot-anchored transform is `pivot - rotation*(scale*pivot)`. A `Vec3::splat(s)`
/// scale is the uniform case; the string legs pass a Y-only stretch so the cord
/// lengthens without fattening its cross-section.
fn pivot_transform(pivot: Vec3, rotation: Quat, scale: Vec3) -> Transform {
    Transform {
        translation: pivot - rotation * (scale * pivot),
        rotation,
        scale,
    }
}

// pub(super) so `slash_trail` can sample the exact blade path at arbitrary swing
// phases and trace its ribbon behind the real sword.
pub(super) fn held_item_local_transform(
    model: ItemModel,
    held_mesh: HeldMesh,
    swing_fraction: f32,
    swap_fraction: f32,
    ranged: RangedPoseInputs,
) -> Transform {
    let phase = swing_fraction.clamp(0.0, 1.0);
    let model_down_offset = match model {
        ItemModel::Bag
        | ItemModel::Deployable
        | ItemModel::Bow
        | ItemModel::Crossbow
        | ItemModel::ThrownBomb => HELD_ITEM_DOWN_OFFSET,
        ItemModel::Hatchet
        | ItemModel::Pickaxe
        | ItemModel::Club
        | ItemModel::Spear
        | ItemModel::Sword
        | ItemModel::Mace => HELD_ITEM_DOWN_OFFSET - 0.03,
    };

    // The head-in-X-plane glbs (tools + weapons + the ranged glbs) get the shared
    // quarter-turn yaw to face the head/blade/limbs forward; the mallet override
    // below undoes it for the one-handed blunt weapons (club, mace) whose head
    // strikes along local Z.
    let head_forward_yaw = Quat::from_rotation_y(PI * 0.5);
    // Orientation fix for the two ranged glbs. Both are authored in a rest pose
    // pointing the WRONG way once exported, so each gets its own whole-item
    // rotation to face down the aim (view -Z), NOT the shared head-forward yaw:
    //
    // - Bow: authored so its shooting axis is authoring -X (arrow flies -X, nock
    //   pulls +X toward the archer) and its limbs run along authoring Z (vertical).
    //   The export maps authoring (x,y,z) -> in-game (x, z, -y), so the raw bow's
    //   aim points in-game -X and its limbs are already vertical (+Y). A -90 deg
    //   yaw about the (vertical) Y axis swings the aim from -X to -Z (down the aim)
    //   while keeping the limbs vertical, and a slight lift tilt raises it toward
    //   eye line. The old +90 yaw pointed it the wrong way (owner's report).
    // - Crossbow: the bolt flies authoring +Z -> in-game +Y (raw muzzle points
    //   straight UP, owner's report). A -90 deg pitch about X drops the muzzle from
    //   +Y to -Z so it points forward down-range, roughly level.
    let bow_model_rotation = Quat::from_rotation_y(-PI * 0.5);
    // The crossbow's stock runs along the model +Y axis (butt at -Y near the
    // trigger, muzzle/prod at +Y), the bolt flying +Y. In-game that means the raw
    // muzzle points straight UP (owner's report). A clean -90 deg rotation about
    // the view X axis pitches the muzzle from +Y down to view -Z, so the whole
    // crossbow lies flat running FORWARD into the screen (foreshortened: prod at
    // the far end down-range, butt back by the hand) and level, with the prod limbs
    // (authored spanning model X) staying screen-horizontal. The extra half-turn
    // roll about the stock (model Y, applied first) puts the crossbow
    // RIGHT-SIDE-UP: without it the groove / string / nut / bolt rendered on the
    // underside and the trigger block loomed on top of the rail, reading as a
    // bulky fake "sight" that hid the ADS target (owner report). The small aim
    // lift and the reload dip are added on top by `crossbow_pose`.
    let crossbow_model_rotation = Quat::from_rotation_x(-PI * 0.5) * Quat::from_rotation_y(PI);
    // The stone spear is authored as a vertical shaft: haft along local +Y with
    // the point at the top (local y up to ~0.65). The shared head-forward yaw only
    // spins it about the vertical axis, leaving it standing straight up (an idle
    // hold, not a thrust). Lay it COUCHED down the aim instead: a -90 deg rotation
    // about the view X axis tips local +Y (the point) forward to view -Z, so the
    // spear points at the crosshair with foreshortening and the butt sits back by
    // the hip. A small downward pitch drops the far tip toward the aim point rather
    // than the sky. `spear_swing_pose`'s forward offset then drives the thrust
    // extension along that couched line.
    let spear_model_rotation = Quat::from_rotation_x(-PI * 0.5 + 0.07);
    let (pose, model_rotation): (ToolSwingPose, Quat) = match model {
        // Ranged weapons don't swing: they draw / reload / fire. The bow holds a
        // draw (or plays its release flick), the crossbow sits shouldered with a
        // recoil kick and a reload crank. The per-piece animator bends the limbs /
        // string; these whole-item poses only aim + carry the rig.
        ItemModel::Bow => {
            let pose = if ranged.drawing {
                bow_draw_pose(ranged.draw_fraction, ranged.time_seconds)
            } else {
                bow_release_pose(ranged.release_progress)
            };
            (pose, bow_model_rotation)
        }
        ItemModel::Crossbow => (
            crossbow_pose(ranged.recoil, ranged.reload_fraction, ranged.aim),
            crossbow_model_rotation,
        ),
        // Bag / deployable-in-hand keep the idle bag hold.
        ItemModel::Bag | ItemModel::Deployable => (bag_idle_pose(phase), Quat::IDENTITY),
        // The thrown bomb: while the toss swing is live it plays the overhand
        // lob off the swing phase (primed at the release beat by the charge
        // release); otherwise the hold-to-charge wind-up pose tracks the
        // charge fraction (and its post-cancel settle), which at zero is the
        // same carry rest the lob starts from. No head to face forward, so no
        // yaw.
        ItemModel::ThrownBomb => {
            let pose = if phase > 0.0 {
                throw_lob_pose(phase)
            } else {
                throw_charge_pose(ranged.throw_wind_up, ranged.time_seconds)
            };
            (pose, Quat::IDENTITY)
        }
        ItemModel::Hatchet => (hatchet_swing_pose(phase), head_forward_yaw),
        ItemModel::Pickaxe => (pickaxe_swing_pose(phase), head_forward_yaw),
        ItemModel::Club => (club_swing_pose(phase), head_forward_yaw),
        ItemModel::Spear => (spear_swing_pose(phase), spear_model_rotation),
        ItemModel::Sword => (sword_swing_pose(phase), head_forward_yaw),
        ItemModel::Mace => (mace_swing_pose(phase), head_forward_yaw),
    };
    // The hatchet/pickaxe glbs carry their blade in the X plane, so the
    // shared quarter-turn yaw above faces it forward. The mallet's head
    // strikes along its local Z instead: skip the yaw, and give it only a
    // gentle forward tip so it stands in the hand like the hatchet does
    // (haft near vertical) while the striking face still points at what
    // the player is about to hit. Keyed on the carry archetype (data), not
    // the mesh variant, so this stays generic across meshes.
    let mallet_grip = matches!(held_mesh.grip(), HeldGrip::Mallet);
    let model_rotation = if mallet_grip {
        Quat::from_rotation_x(-0.35)
    } else {
        model_rotation
    };

    // The mallet is a short one-handed tool, not a long two-handed one, so it
    // sits closer to the player: pull it back toward the camera (much less
    // forward) and drop it a touch, reading as a relaxed one-arm carry rather
    // than a weapon held out front. The wooden club is the exception within the
    // mallet family: its head is far bulkier than the hammer's, so pulled that
    // close it filled the frame (owner report: too big); it keeps most of the
    // forward distance instead.
    let model_offset = if matches!(model, ItemModel::Club) {
        Vec3::new(0.02, -0.05, 0.04)
    } else if mallet_grip {
        Vec3::new(0.0, -0.03, 0.20)
    } else if matches!(model, ItemModel::Sword) {
        // The sword is a long, close one-hander carried low on the RIGHT of the
        // frame like the other melee weapons. The grip seat below already pushes
        // the mesh up-and-right along the blade axis, so this offset pulls the
        // hand point back DOWN (`-Y`) and slightly in (`-X`) so the wrapped grip
        // rides low-right and the foreshortened blade angles up toward the frame
        // centre without dominating it. It used to ride high and near the centre,
        // which read as floating in front of the face rather than carried.
        Vec3::new(0.02, -0.12, 0.0)
    } else if matches!(model, ItemModel::Spear) {
        // The couched spear runs forward from the hip, biased right (`+X`) so
        // the hand reads on the right, with a slight lift (`+Y`). The mid-shaft
        // grip seat below already slides the mesh ~0.22 toward the camera along
        // the shaft, so only a small `+Z` pull remains here; the two together
        // put the visible shaft entering from the lower-right with the butt
        // running off-frame behind the hand.
        Vec3::new(0.10, 0.03, 0.08)
    } else if matches!(model, ItemModel::Crossbow) {
        // Shoulder the crossbow: the clean model rotation lays the stock running
        // forward into the screen (muzzle/prod at the far end, butt back by the
        // hand), so it reads correctly foreshortened. Lift it up toward the eye
        // line (`+Y`) and pull it in from the right edge (`-X`) so the stock rides
        // at the shoulder and the prod sits up near the crosshair. The generous
        // `+Z` pull seats the butt right back at the shoulder: at the earlier
        // small pull the whole crossbow floated out ahead of the camera, reading
        // as held one-handed at full arm stretch instead of braced. Holding the
        // aim (right mouse) slides the stock the rest of the way onto the
        // CENTRE line and up UNDER the eye, the ADS sight picture: at full aim
        // the `-X` exactly cancels the base right offset. The aimed lift puts
        // the eye right at the authored sight plane (rear-notch + front-post
        // tops, authoring y 0.080 over the stock line), so the rail reads
        // nearly edge-on, most of the viewmodel foreshortens away, and the
        // shooter aims with the irons (owner spec: flat against the screen,
        // just the iron sight, with the bolt line just below it).
        let carry = Vec3::new(-0.16, 0.10, 0.20);
        let aimed = Vec3::new(-HELD_ITEM_RIGHT_OFFSET, 0.155, 0.26);
        carry.lerp(aimed, smoothstep(ranged.aim.clamp(0.0, 1.0)))
    } else if matches!(model, ItemModel::Bow) {
        // The bow rests on the RIGHT side of the frame (only a small `-X` pull
        // off the base right offset, not the old deep pull toward centre) and a
        // touch down, so the carry reads as a bow held ready at the archer's
        // side. The draw pose then pulls it inward toward the eye rather than
        // sliding it across to the left (owner feedback: the draw must animate
        // inward-right, and the carry belongs on the right, not too far out).
        Vec3::new(-0.08, -0.06, 0.0)
    } else {
        Vec3::ZERO
    };

    // Per-item grip seat, expressed in the ITEM'S OWN frame and applied after
    // the whole-item rotation: it slides the mesh along itself so the hand
    // grips the right part of the handle (the sword's short wrap, the spear's
    // mid-shaft) whatever the carry rotation is doing.
    let (model_rotation, grip_seat) = if matches!(model, ItemModel::Sword) {
        // Hold the blade UPRIGHT at guard (owner feedback: upright, not laid
        // toward the target): near-vertical on screen with only a whisper of
        // rightward lean and away-tilt so it doesn't read as a flat cardboard
        // cutout. Axis note: the head-forward yaw (about Y) swaps the pre-yaw
        // local axes, so a pre-yaw X tilt becomes the on-screen LEFT/RIGHT lean
        // (positive = right) and a pre-yaw Z tilt becomes the away-from-camera
        // foreshortening (negative = away).
        let tilt = Quat::from_rotation_x(0.10) * Quat::from_rotation_z(-0.14);
        // Seat the handle centre at the hand: the handle spans local y [-0.5, -0.28]
        // (centre ~ -0.39) and the blade runs from -0.53 up to +0.35. Translate the
        // mesh by +0.44 along its local Y so a point just above the handle centre
        // lands at the base offset, i.e. the hand grips the wrap and the pommel
        // hangs just below it, both on screen.
        (model_rotation * tilt, Vec3::new(0.0, 0.44, 0.0))
    } else if matches!(model, ItemModel::Spear) {
        // Slide the spear mesh backward along its own shaft so the hand sits at
        // MID-SHAFT: the butt runs off past the bottom-right frame edge instead
        // of ending right at the hand, which read as the spear being held by
        // its very bottom (owner report). Expressed in the spear's local frame
        // (negative Y = toward the butt), rotated with the couched shaft.
        (model_rotation, Vec3::new(0.0, -0.22, 0.0))
    } else {
        (model_rotation, Vec3::ZERO)
    };

    let swing_translation = Vec3::NEG_Z * pose.forward + Vec3::X * pose.right + Vec3::Y * pose.up;
    let base_rotation = Quat::from_euler(EulerRot::XYZ, pose.pitch, pose.yaw, pose.roll);
    let base_quat = base_rotation * model_rotation;
    // The grip seat is in the item's local frame, so rotate it by the whole-item
    // rotation before adding it to the view-space translation.
    let seat_translation = base_quat * grip_seat;
    let base_translation = Vec3::NEG_Z * HELD_ITEM_FORWARD_OFFSET
        + Vec3::X * HELD_ITEM_RIGHT_OFFSET
        - Vec3::Y * model_down_offset
        + model_offset
        + seat_translation
        + swing_translation;

    // The club renders slightly under true scale in first person: even pushed
    // out to the mallet family's carry distance its bulky head dominated the
    // frame (owner report: too big). Viewmodel only; the third-person club on
    // remote rigs stays authored size.
    let viewmodel_scale = if matches!(model, ItemModel::Club) {
        Vec3::splat(0.82)
    } else {
        Vec3::ONE
    };

    // Entry animation: the tool is "picked off the player's back", it
    // starts below the rest pose and slightly tilted forward, then eases up
    // into place. Heavier items (pickaxe) drop further and tilt more so the
    // lift reads as weightier without being noticeably slower.
    let swap = swap_fraction.clamp(0.0, 1.0);
    let lag = 1.0 - smoothstep(swap);
    if lag <= f32::EPSILON {
        return Transform::from_translation(base_translation)
            .with_rotation(base_quat)
            .with_scale(viewmodel_scale);
    }

    let (drop, back, pitch_lag) = match model {
        // Bag, deployable-in-hand, and the thrown bomb are light held bundles
        // that lift the same gentle way.
        ItemModel::Bag | ItemModel::Deployable | ItemModel::ThrownBomb => (0.40, 0.04, -0.30),
        // The club and sword are hatchet-weight one/two-handers; the spear is
        // similar; they all lift like the hatchet, and so does the bow (a light
        // wooden two-hander). The mace and the crossbow are the heaviest carries,
        // so they drop and tilt the most, like the pickaxe; the crossbow's slow
        // shouldering matches its SWAP_DURATION_PICKAXE cadence in gather.rs.
        ItemModel::Hatchet
        | ItemModel::Club
        | ItemModel::Spear
        | ItemModel::Sword
        | ItemModel::Bow => (0.50, 0.05, -0.40),
        ItemModel::Pickaxe | ItemModel::Mace | ItemModel::Crossbow => (0.68, 0.06, -0.55),
    };

    let enter_offset = Vec3::new(0.0, -drop * lag, back * lag);
    let enter_tilt = Quat::from_rotation_x(pitch_lag * lag);
    Transform::from_translation(base_translation + enter_offset)
        .with_rotation(enter_tilt * base_quat)
        .with_scale(viewmodel_scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranged(draw: f32, drawing: bool) -> RangedPoseInputs {
        RangedPoseInputs {
            draw_fraction: draw,
            drawing,
            ..Default::default()
        }
    }

    #[test]
    fn static_pieces_have_an_identity_local_transform() {
        // Every melee / tool layer, plus the bow grip and the crossbow stock /
        // iron, is a static piece: its per-piece transform is identity, so the
        // whole-item transform is the entire transform (single-layer items are
        // byte-unchanged).
        for (model, slot) in [
            (ItemModel::Sword, HeldPieceSlot::Static),
            (ItemModel::Hatchet, HeldPieceSlot::Static),
            (ItemModel::Bow, HeldPieceSlot::Static),
            (ItemModel::Crossbow, HeldPieceSlot::Static),
            // A mismatched slot (a bow limb slot on a non-bow) also falls through to
            // identity, so a stale tag can never corrupt a static item.
            (ItemModel::Sword, HeldPieceSlot::BowLimbUpper),
        ] {
            let t = held_piece_local_transform(model, slot, RangedPoseInputs::default());
            assert_eq!(t.translation, Vec3::ZERO, "{model:?}/{slot:?} no translate");
            assert!(
                t.rotation.angle_between(Quat::IDENTITY) < 1e-6,
                "{model:?}/{slot:?} no rotate"
            );
        }
    }

    #[test]
    fn bow_arrow_rides_the_string_nock_and_collapses_after_loose() {
        // At rest the nocked arrow sits exactly as authored (a ready arrow
        // always shows on the carried bow).
        let rest =
            held_piece_local_transform(ItemModel::Bow, HeldPieceSlot::BowArrow, ranged(0.0, true));
        assert!(
            rest.translation.length() < 1e-6,
            "authored at the rest nock"
        );
        assert!(
            (rest.scale - Vec3::ONE).length() < 1e-6,
            "full size at rest"
        );

        // At full draw it slides rigidly with the drawn nock (straight back
        // toward the archer in the bow's string plane), so its exposed tip
        // reads as the aim reference.
        let full =
            held_piece_local_transform(ItemModel::Bow, HeldPieceSlot::BowArrow, ranged(1.0, true));
        let expected = BOW_VIEWMODEL_FULL_NOCK - bow_rig::from_authoring(0.16, 0.0, 0.0);
        assert!(
            (full.translation - expected).length() < 1e-5,
            "the arrow slides back exactly with the string nock"
        );
        assert!(
            full.rotation.angle_between(Quat::IDENTITY) < 1e-6,
            "a rigid slide, never a re-aim"
        );

        // Right after loose the piece collapses (the real arrow is flying
        // down-range), then grows back in over the tail of the release flick.
        let just_loosed = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowArrow,
            RangedPoseInputs {
                drawing: false,
                release_progress: 0.1,
                ..Default::default()
            },
        );
        assert!(
            just_loosed.scale.length() < 1e-6,
            "the nocked arrow is gone right after loose"
        );
        let renocked = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowArrow,
            RangedPoseInputs {
                drawing: false,
                release_progress: 1.0,
                ..Default::default()
            },
        );
        assert!(
            (renocked.scale - Vec3::ONE).length() < 1e-6,
            "a settled release shows the next arrow nocked"
        );
    }

    #[test]
    fn crossbow_bolt_shows_only_while_cocked_and_rides_the_string() {
        // Idle (no recoil, no reload) is cocked: the bolt sits full size at its
        // authored spot, glued to the latched string.
        let idle = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs::default(),
        );
        assert!(
            (idle.scale - Vec3::ONE).length() < 1e-6,
            "cocked shows the bolt"
        );
        assert!(
            idle.translation.length() < 1e-6,
            "authored at the cocked nut"
        );

        // Just fired (string snapped forward): the bolt is gone, the real
        // projectile is flying.
        let fired = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                recoil: 1.0,
                ..Default::default()
            },
        );
        assert!(fired.scale.length() < 1e-6, "no bolt right after the shot");

        // Mid-reload: still no bolt until the crank is nearly done.
        let cranking = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                reload_fraction: 0.5,
                ..Default::default()
            },
        );
        assert!(cranking.scale.length() < 1e-6, "no bolt mid-crank");

        // Reload complete: the next bolt is seated.
        let ready = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowBolt,
            RangedPoseInputs {
                reload_fraction: 1.0,
                ..Default::default()
            },
        );
        assert!(
            (ready.scale - Vec3::ONE).length() < 1e-6,
            "a finished reload seats the next bolt"
        );
    }

    #[test]
    fn bow_limbs_flex_back_toward_the_archer_as_the_draw_ramps() {
        // At rest both limbs are unflexed (identity); at full draw each rotates by
        // the authored flex about its pivot, in OPPOSITE directions (upper +flex,
        // lower -flex), so the bow bends symmetrically with both tips curling
        // BACK toward the archer (the string side).
        let rest_upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbUpper,
            ranged(0.0, true),
        );
        assert!(
            rest_upper.rotation.angle_between(Quat::IDENTITY) < 1e-6,
            "an undrawn bow limb is unflexed"
        );

        let full_upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbUpper,
            ranged(1.0, true),
        );
        let full_lower = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowLimbLower,
            ranged(1.0, true),
        );
        // Both limbs rotate by the full flex magnitude at full draw.
        assert!(
            (full_upper.rotation.angle_between(Quat::IDENTITY) - BOW_LIMB_FLEX).abs() < 1e-4,
            "the upper limb flexes by the authored angle at full draw"
        );
        assert!(
            (full_lower.rotation.angle_between(Quat::IDENTITY) - BOW_LIMB_FLEX).abs() < 1e-4,
            "the lower limb flexes by the authored angle at full draw"
        );
        // They flex in opposite directions (mirror), so the two rotations are not
        // equal.
        assert!(
            full_upper.rotation.angle_between(full_lower.rotation) > BOW_LIMB_FLEX,
            "the limbs flex in opposite directions (mirror)"
        );
        // Direction check (owner report: the stave used to bend the WRONG way,
        // toward the target): at full draw each tip must land further along
        // +X (toward the archer / string side) than its authored rest tip.
        for upper in [true, false] {
            let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
            let flexed = flexed_limb_tip(1.0, upper);
            assert!(
                flexed.x > rest_tip.x + 0.05,
                "the limb tip curls back toward the archer (upper={upper})"
            );
        }
    }

    #[test]
    fn bow_string_legs_anchor_on_the_flexed_limb_tips() {
        // The string tracks the BENT limb, not the rest stave: each leg's anchored
        // (tip) end must land on the flexed limb tip at full draw, so the limb flex
        // reads (the string ends ride inward with the tips instead of floating off
        // the un-bent rest tips). Applying the leg transform to its authored rest tip
        // must reproduce the flexed tip.
        for upper in [true, false] {
            let rest_tip = bow_rig::from_authoring(0.16, 0.0, if upper { 0.45 } else { -0.45 });
            let flexed = flexed_limb_tip(1.0, upper);
            // The flex actually moves the tip (a bent limb, not a straight one).
            assert!(
                flexed.distance(rest_tip) > 0.02,
                "the limb tip visibly flexes inward under load (upper={upper})"
            );
            let leg = held_piece_local_transform(
                ItemModel::Bow,
                if upper {
                    HeldPieceSlot::BowStringUpper
                } else {
                    HeldPieceSlot::BowStringLower
                },
                ranged(1.0, true),
            );
            let anchored_end = leg.transform_point(rest_tip);
            assert!(
                anchored_end.distance(flexed) < 1e-4,
                "the string leg's anchored end welds to the flexed limb tip (upper={upper})"
            );
        }
    }

    #[test]
    fn bow_string_forms_a_deep_v_toward_the_archer_at_full_draw() {
        // The two string legs are pinned at their limb tips and meet at the shared
        // nock. At full draw the viewmodel pulls the nock back toward the archer
        // (along +X, which maps to view +Z toward the camera) AND down toward the
        // anchor (along -Y, which maps to view -Y), so applying each leg's transform
        // to the rest nock must land BOTH legs' free ends at the SAME drawn nock,
        // that drawn nock must be clearly displaced from the rest nock (a deep pull
        // toward the eye), and the two legs must splay into a genuine V (not a
        // straight line). The pull stays in the bow's flat laterally (Z = 0); the
        // +X brings the nock toward the camera and the -Y drops it toward the cheek.
        let rest_nock = bow_nock_point(0.0);
        let drawn_nock = bow_nock_point(1.0);

        let upper = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowStringUpper,
            ranged(1.0, true),
        );
        let lower = held_piece_local_transform(
            ItemModel::Bow,
            HeldPieceSlot::BowStringLower,
            ranged(1.0, true),
        );
        // Each leg is pinned at its limb tip and rotated + length-scaled so its
        // free (nock) end REACHES the drawn nock exactly (the string stays
        // connected to the arrow nock through the draw). Apply each leg's transform
        // to the rest nock (its authored free end) and confirm it lands on the
        // drawn nock.
        let upper_free = upper.transform_point(rest_nock);
        let lower_free = lower.transform_point(rest_nock);
        assert!(
            upper_free.distance(drawn_nock) < 1e-4,
            "the upper string leg reaches the drawn nock"
        );
        assert!(
            lower_free.distance(drawn_nock) < 1e-4,
            "the lower string leg reaches the drawn nock"
        );
        // Both free ends meet at the same point (the V apex), forming the string V.
        assert!(
            upper_free.distance(lower_free) < 1e-4,
            "the two string legs meet at a single nock"
        );
        // The drawn nock is pulled a real distance toward the archer (a readable V,
        // not a shallow twitch), and the whole pull stays IN THE BOW'S STRING
        // PLANE (no lateral Z component): an out-of-plane pull made the string
        // appear to reach for the player independently of the bow's rotation.
        assert!(
            drawn_nock.distance(rest_nock) > 0.2,
            "the nock pulls clearly toward the archer at full draw"
        );
        assert!(
            drawn_nock.x > rest_nock.x + 0.15,
            "the drawn nock pulls back toward the archer along the bow's +X"
        );
        assert!(
            drawn_nock.z.abs() < 1e-6,
            "the pull stays in the bow's string plane, anchored to the stave"
        );
        // The two legs are genuinely splayed (not collinear): from the shared apex
        // (drawn nock) the direction to the upper limb tip and to the lower limb tip
        // are well apart, so the string reads as a V rather than a single line. The
        // legs anchor at the FLEXED tips (where the limbs bent to under load), not
        // the rest tips, so the splay is measured from those.
        let upper_tip = flexed_limb_tip(1.0, true);
        let lower_tip = flexed_limb_tip(1.0, false);
        let to_upper = (upper_tip - drawn_nock).normalize_or_zero();
        let to_lower = (lower_tip - drawn_nock).normalize_or_zero();
        let cos = to_upper.dot(to_lower);
        assert!(
            cos > -0.9 && cos < 0.6,
            "the two string legs splay into a V (not a straight line); cos = {cos}"
        );
    }

    #[test]
    fn crossbow_string_snaps_forward_on_release_and_sits_back_when_cocked() {
        // Cocked (ready, no recoil / reload): the string nut sits back near the
        // trigger. On a fresh shot (recoil 1) it snaps forward toward the prod. The
        // down-range axis is in-game +Y (authoring +Z), so "forward" is a larger y.
        let cocked = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs::default(),
        );
        let fired = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs {
                recoil: 1.0,
                ..Default::default()
            },
        );
        assert!(
            fired.translation.y > cocked.translation.y + 0.1,
            "the string snaps forward toward the prod on release"
        );
        // Mid-reload the nut is drawn back from the released position toward cocked.
        let mid_reload = held_piece_local_transform(
            ItemModel::Crossbow,
            HeldPieceSlot::CrossbowString,
            RangedPoseInputs {
                reload_fraction: 0.5,
                ..Default::default()
            },
        );
        assert!(
            mid_reload.translation.y < fired.translation.y,
            "the reload crank draws the string back from the fired position"
        );
    }

    #[test]
    fn sword_grip_offset_lifts_the_handle_into_frame() {
        // The sword's short grip needs a per-item offset so the handle shows: at
        // rest the sword must sit higher (and pulled back toward the camera) than
        // it would without the offset. Compare against the hatchet, which uses the
        // shared LongHafted layout with no such offset, at the same rest pose: the
        // sword sits higher (+Y) and less far forward (+Z) than the raw base.
        let sword = held_item_local_transform(
            ItemModel::Sword,
            HeldMesh::IronSword,
            0.0,
            1.0,
            RangedPoseInputs::default(),
        );
        let hatchet = held_item_local_transform(
            ItemModel::Hatchet,
            HeldMesh::IronHatchet,
            0.0,
            1.0,
            RangedPoseInputs::default(),
        );
        // The sword model-down offset matches the other long weapons, so any
        // divergence is the deliberate grip seat + tilt. It must sit clearly
        // higher than the raw LongHafted placement (so the handle clears the bottom
        // of the frame) and be measurably re-seated overall.
        assert!(
            sword.translation.y > hatchet.translation.y + 0.05,
            "the sword grip is seated higher so the handle clears the frame bottom"
        );
        assert!(
            sword.translation.distance(hatchet.translation) > 0.1,
            "the sword is re-seated off the raw LongHafted placement"
        );
        // The sword also carries its own subtle upright tilt (a whisper of
        // right-lean + away-tilt), so its rest rotation diverges from the
        // hatchet's, without ever laying the blade toward the target.
        assert!(
            sword.rotation.angle_between(hatchet.rotation) > 0.1,
            "the sword has its own upright carry tilt"
        );
    }

    #[test]
    fn fully_swapped_in_tool_sits_at_its_rest_pose() {
        // swap_fraction == 1.0 means the tool has finished lifting into
        // view, so no enter-offset is applied, the transform is the
        // canonical rest pose for the model.
        let rest = held_item_local_transform(
            ItemModel::Hatchet,
            HeldMesh::StoneHatchet,
            0.0,
            1.0,
            RangedPoseInputs::default(),
        );

        // The base rest translation sits forward (-Z), right (+X) and down.
        assert!(rest.translation.z < 0.0, "held item is in front of camera");
        assert!(rest.translation.x > 0.0, "held item offset to the right");
        assert!(rest.translation.y < 0.0, "held item offset downward");
    }

    /// Rest-pose transform for a melee / tool item (neutral ranged inputs), so the
    /// existing tests read as before the ranged parameter was added.
    fn melee_transform(model: ItemModel, mesh: HeldMesh, swing: f32, swap: f32) -> Transform {
        held_item_local_transform(model, mesh, swing, swap, RangedPoseInputs::default())
    }

    #[test]
    fn entry_animation_drops_and_tilts_the_item_below_its_rest_pose() {
        // At swap_fraction == 0.0 the tool is freshly "picked off the
        // back", it starts lower than the rest pose.
        let entering = melee_transform(ItemModel::Pickaxe, HeldMesh::StonePickaxe, 0.0, 0.0);
        let rest = melee_transform(ItemModel::Pickaxe, HeldMesh::StonePickaxe, 0.0, 1.0);
        assert!(
            entering.translation.y < rest.translation.y,
            "entering item starts below rest"
        );
        // And it's tilted relative to rest.
        assert!(entering.rotation.angle_between(rest.rotation) > 0.05);
    }

    #[test]
    fn heavier_pickaxe_drops_further_on_entry_than_the_bag() {
        let pickaxe = melee_transform(ItemModel::Pickaxe, HeldMesh::StonePickaxe, 0.0, 0.0);
        let bag = melee_transform(ItemModel::Bag, HeldMesh::Bag, 0.0, 0.0);
        // The pickaxe's entry drop is the largest of the three models, so at
        // the start of the swap it sits lower than the bag.
        assert!(pickaxe.translation.y < bag.translation.y);
    }

    #[test]
    fn swing_phase_moves_the_held_item_relative_to_idle() {
        // A mid-swing phase displaces the hatchet from its idle (phase 0)
        // pose, the swing animation actually drives the transform.
        let idle = melee_transform(ItemModel::Hatchet, HeldMesh::StoneHatchet, 0.0, 1.0);
        let mid = melee_transform(ItemModel::Hatchet, HeldMesh::StoneHatchet, 0.5, 1.0);
        assert!(idle.translation.distance(mid.translation) > 0.01);
    }

    #[test]
    fn bow_draw_drives_the_viewmodel_off_the_draw_fraction() {
        // The bow is no longer a placeholder bag hold: holding a draw at fraction 1
        // must produce a clearly different viewmodel transform than rest (fraction
        // 0), driven by the real draw pose.
        let rest = held_item_local_transform(
            ItemModel::Bow,
            HeldMesh::WoodenBow,
            0.0,
            1.0,
            RangedPoseInputs {
                drawing: true,
                draw_fraction: 0.0,
                ..Default::default()
            },
        );
        let full = held_item_local_transform(
            ItemModel::Bow,
            HeldMesh::WoodenBow,
            0.0,
            1.0,
            RangedPoseInputs {
                drawing: true,
                draw_fraction: 1.0,
                ..Default::default()
            },
        );
        assert!(
            rest.translation.distance(full.translation) > 0.05
                || rest.rotation.angle_between(full.rotation) > 0.05,
            "the bow viewmodel tracks the draw fraction"
        );
    }

    #[test]
    fn crossbow_recoil_and_reload_drive_the_viewmodel() {
        // The crossbow is no longer a placeholder: a fresh fire (recoil 1) and a
        // mid-reload crank (reload 0.5) must each move the viewmodel off the
        // settled ready pose.
        let ready = held_item_local_transform(
            ItemModel::Crossbow,
            HeldMesh::Crossbow,
            0.0,
            1.0,
            RangedPoseInputs::default(),
        );
        let fired = held_item_local_transform(
            ItemModel::Crossbow,
            HeldMesh::Crossbow,
            0.0,
            1.0,
            RangedPoseInputs {
                recoil: 1.0,
                ..Default::default()
            },
        );
        let cranking = held_item_local_transform(
            ItemModel::Crossbow,
            HeldMesh::Crossbow,
            0.0,
            1.0,
            RangedPoseInputs {
                reload_fraction: 0.5,
                ..Default::default()
            },
        );
        assert!(
            ready.translation.distance(fired.translation) > 0.02
                || ready.rotation.angle_between(fired.rotation) > 0.02,
            "recoil moves the crossbow viewmodel"
        );
        assert!(
            ready.translation.distance(cranking.translation) > 0.02
                || ready.rotation.angle_between(cranking.rotation) > 0.02,
            "the reload crank moves the crossbow viewmodel"
        );
    }

    #[test]
    fn every_item_model_pose_dispatches_and_animates() {
        // Completeness check for pose dispatch: every ItemModel must resolve to a
        // pose that actually moves the held item between two states, so no
        // archetype animates as a dead frame. Melee/tool archetypes animate off the
        // swing fraction; the ranged archetypes (Bow / Crossbow) are NO LONGER
        // placeholder bag holds, they drive off their own draw / reload inputs, so
        // each is exercised with the input that actually moves it. The exhaustive
        // `match` on `model` forces a new variant to pick a representative mesh +
        // state pair here, matching the dispatch in `held_item_local_transform`.
        // Paired with the duration/impact-fraction completeness test in
        // `state::gather::tests`.
        let all_models = [
            ItemModel::Bag,
            ItemModel::Deployable,
            ItemModel::Hatchet,
            ItemModel::Pickaxe,
            ItemModel::Club,
            ItemModel::Spear,
            ItemModel::Sword,
            ItemModel::Mace,
            ItemModel::Bow,
            ItemModel::Crossbow,
            ItemModel::ThrownBomb,
        ];
        for model in all_models {
            // A representative held mesh + the two pose states to compare, per
            // archetype. The exhaustive match makes a new ItemModel a compile error
            // until it is covered here.
            let (mesh, state_a, state_b): (HeldMesh, RangedPoseInputs, RangedPoseInputs) =
                match model {
                    ItemModel::Bag | ItemModel::Deployable => (
                        HeldMesh::Bag,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Hatchet => (
                        HeldMesh::StoneHatchet,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Pickaxe => (
                        HeldMesh::StonePickaxe,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Club => (
                        HeldMesh::WoodenClub,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Spear => (
                        HeldMesh::StoneSpear,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Sword => (
                        HeldMesh::IronSword,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    ItemModel::Mace => (
                        HeldMesh::IronMace,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                    // The bow animates off its draw fraction (rest draw vs full draw).
                    ItemModel::Bow => (
                        HeldMesh::WoodenBow,
                        RangedPoseInputs {
                            drawing: true,
                            draw_fraction: 0.0,
                            ..Default::default()
                        },
                        RangedPoseInputs {
                            drawing: true,
                            draw_fraction: 1.0,
                            ..Default::default()
                        },
                    ),
                    // The crossbow animates off its recoil / reload (ready vs fired).
                    ItemModel::Crossbow => (
                        HeldMesh::Crossbow,
                        RangedPoseInputs::default(),
                        RangedPoseInputs {
                            recoil: 1.0,
                            ..Default::default()
                        },
                    ),
                    // The thrown bomb animates off the toss (swing) fraction, like
                    // the melee archetypes, so its two states are the default ranged
                    // inputs and the swing fraction does the moving below.
                    ItemModel::ThrownBomb => (
                        HeldMesh::PowderBomb,
                        RangedPoseInputs::default(),
                        RangedPoseInputs::default(),
                    ),
                };
            // Melee/tool archetypes move off the swing fraction (0.0 -> 0.5); the
            // ranged archetypes hold swing fraction at 0 and move off their state
            // inputs instead.
            let is_ranged = matches!(model, ItemModel::Bow | ItemModel::Crossbow);
            let (swing_a, swing_b) = if is_ranged { (0.0, 0.0) } else { (0.0, 0.5) };
            let rest = held_item_local_transform(model, mesh, swing_a, 1.0, state_a);
            let mid = held_item_local_transform(model, mesh, swing_b, 1.0, state_b);
            assert!(
                rest.translation.distance(mid.translation) > 1e-4
                    || rest.rotation.angle_between(mid.rotation) > 1e-4,
                "{model:?} pose must move between its two states"
            );
        }
    }
}
