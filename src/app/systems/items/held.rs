//! First-person held-item viewmodel: the generic layer spawning, materials,
//! idle sway, grips, and whole-item swing/carry transform. The weapon-specific
//! per-piece rigs (bow draw/limb/string/arrow, crossbow cock/string/bolt, the
//! bandage tail) live in the `ranged_viewmodel` sibling and are re-exported
//! flat, so call sites keep saying `held::X` / `items::X`.

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
    ToolSwingPose, bag_idle_pose, bandage_use_pose, bow_draw_pose, bow_release_pose,
    club_swing_pose, crossbow_pose, hatchet_swing_pose, mace_swing_pose, pickaxe_swing_pose,
    sickle_swing_pose, smoothstep, spear_swing_pose, sword_swing_pose, throw_charge_pose,
    throw_lob_pose,
};

mod ranged_viewmodel;

pub(crate) use ranged_viewmodel::{
    BOW_RELEASE_SECONDS, CROSSBOW_RECOIL_SECONDS, RangedPoseInputs, held_piece_local_transform,
};

const HELD_ITEM_FORWARD_OFFSET: f32 = 0.62;
const HELD_ITEM_RIGHT_OFFSET: f32 = 0.28;
const HELD_ITEM_DOWN_OFFSET: f32 = 0.24;

#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn apply_held_item_visual_system(
    mut commands: Commands,
    local_player: Res<LocalPlayerState>,
    menu: Res<MenuState>,
    visuals: Res<HeldItemVisuals>,
    gather_input: Res<GatherInputState>,
    ranged: Res<RangedDrawState>,
    throw_charge: Res<crate::app::state::ThrowChargeState>,
    consume: Res<crate::app::state::ConsumeChargeState>,
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
        use_fraction: consume.use_fraction(),
        use_settle: consume.settle_progress(),
        use_ended_at: consume.ended_at(),
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
        // The bow is authored upright in its own frame: the limbs run along
        // local Y, the string/archer side is local +X, and down-range (the
        // target) is local -X (see `bow_rig`). We want the stave vertical with
        // down-range pointing along the player's forward (-Z), which is a -90°
        // yaw about Y: local -X -> player -Z (forward), local +X -> +Z (back
        // toward the archer), limbs stay vertical. This is the OPPOSITE yaw sign
        // from the hafted tools; the tool's +90° yaw is what rolled the bow the
        // wrong way in third-person. Gripped at the riser (the stave's vertical
        // centre, local Y = 0), so no grip offset.
        HeldGrip::Bow => (Quat::from_rotation_y(-PI * 0.5), 0.0),
        // The spear is authored as a vertical shaft (haft along +Y, point at the
        // top). Lay it COUCHED down the aim: a -90° pitch about X tips local +Y
        // (the point) forward to the player's -Z, so the shaft runs level with the
        // point leading and the butt back. Gripped low toward the butt so the
        // point extends well forward, mirroring the first-person couched carry;
        // the thrust rides the arm extension on top. Matches the FP
        // `spear_model_rotation` (`from_rotation_x(-PI/2 + small)`).
        HeldGrip::Spear => (Quat::from_rotation_x(-PI * 0.5), -0.15),
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
        | ItemModel::ThrownBomb
        | ItemModel::Bandage => HELD_ITEM_DOWN_OFFSET,
        ItemModel::Hatchet
        | ItemModel::Pickaxe
        | ItemModel::Club
        | ItemModel::Spear
        | ItemModel::Sword
        | ItemModel::Mace
        | ItemModel::Sickle => HELD_ITEM_DOWN_OFFSET - 0.03,
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
    // The bandage glb runs its tail along local -Z (straight AWAY from the eye), so
    // unauthored it unrolls into the distance and foreshortens to nothing. Tip it
    // -90 deg about X so the tail hangs DOWN into view, then yaw the roll so its
    // coil face turns toward the camera: the coil is the detail that says
    // "bandage" rather than "log", and it has to be the thing you see.
    let bandage_model_rotation = Quat::from_rotation_y(-0.85) * Quat::from_rotation_x(-PI * 0.5);
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
        // The bandage neither swings nor fires: its entire animation is the use
        // charge. The pose lifts the roll across to the off-hand and turns its coil
        // face toward the camera, and settles back to the carry when the use ends
        // (completed or abandoned). The tail unrolling out of the roll is the
        // per-piece half, in `bandage_tail_transform`. No head to face forward, so
        // no yaw correction.
        ItemModel::Bandage => (
            bandage_use_pose(
                ranged.use_fraction,
                ranged.use_settle,
                ranged.use_ended_at,
                ranged.time_seconds,
            ),
            bandage_model_rotation,
        ),
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
        // The sickle's hook lies in a vertical plane containing the haft
        // (like a hatchet blade), so which way its FACE points is set by the
        // yaw. Composed per the owner's reference framing: handle at the
        // lower RIGHT with the hook arcing up and INWARD (screen-left,
        // toward the crosshair) and the point hanging down; no half-turn, so
        // the authored -X hook sweeps left. The face wants to point at the
        // CAMERA RAY through the carry slot, not down the view axis: the
        // item rides the right edge of the frame, where perspective adds
        // ~15-20 degrees of effective yaw, so only a small trim keeps the
        // face reading. The small X lean tips the hook forward.
        ItemModel::Sickle => (
            sickle_swing_pose(phase),
            head_forward_yaw * Quat::from_rotation_y(0.10) * Quat::from_rotation_x(-0.35),
        ),
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
    } else if matches!(model, ItemModel::Sickle) {
        // Keep the sickle out of the frame centre by pushing it AWAY from the
        // camera (negative Z runs down the view axis): the crescent is a big
        // mesh, and at the shared tool distance it loomed across half the
        // screen. Only a small drop; the leaned carry already sits low.
        Vec3::new(0.02, -0.06, -0.14)
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
    // frame (owner report: too big). The sickle's long crescent has the same
    // problem at the shared tool distance. Viewmodel only; the third-person
    // meshes on remote rigs stay authored size.
    let viewmodel_scale = if matches!(model, ItemModel::Club) {
        Vec3::splat(0.82)
    } else if matches!(model, ItemModel::Sickle) {
        Vec3::splat(0.86)
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
        // Bag, deployable-in-hand, the thrown bomb, and the bandage are light held
        // bundles that lift the same gentle way.
        ItemModel::Bag | ItemModel::Deployable | ItemModel::ThrownBomb | ItemModel::Bandage => {
            (0.40, 0.04, -0.30)
        }
        // The club and sword are hatchet-weight one/two-handers; the spear is
        // similar; they all lift like the hatchet, and so does the bow (a light
        // wooden two-hander). The mace and the crossbow are the heaviest carries,
        // so they drop and tilt the most, like the pickaxe; the crossbow's slow
        // shouldering matches its SWAP_DURATION_PICKAXE cadence in gather.rs.
        ItemModel::Hatchet
        | ItemModel::Club
        | ItemModel::Spear
        | ItemModel::Sword
        | ItemModel::Sickle
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
            ItemModel::Bandage,
            ItemModel::Sickle,
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
                    ItemModel::Sickle => (
                        HeldMesh::Sickle,
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
                    // The bandage animates off its USE charge (carry rest vs the
                    // wrap fully going on). `use_settle: 1.0` on the rest state is
                    // "fully settled at the carry", which is what an idle bandage is.
                    ItemModel::Bandage => (
                        HeldMesh::Bandage,
                        RangedPoseInputs {
                            use_fraction: 0.0,
                            use_settle: 1.0,
                            ..Default::default()
                        },
                        RangedPoseInputs {
                            use_fraction: 1.0,
                            ..Default::default()
                        },
                    ),
                };
            // Melee/tool archetypes move off the swing fraction (0.0 -> 0.5); the
            // ranged archetypes (and the bandage, which charges rather than swings)
            // hold swing fraction at 0 and move off their state inputs instead.
            let is_ranged = matches!(
                model,
                ItemModel::Bow | ItemModel::Crossbow | ItemModel::Bandage
            );
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
