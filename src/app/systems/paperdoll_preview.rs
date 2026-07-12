//! Live 3D character preview for the inventory paperdoll.
//!
//! Spawns one extra copy of the player rig far below the world on a dedicated
//! render layer, dresses it from the LOCAL player's replicated equipment slots
//! (the same `PlayerEquipmentVisual::from_equipment_slots` derivation the
//! server runs for peers) plus the active held item, and renders it with a
//! dedicated off-screen camera into an [`Image`] the inventory tab paints next
//! to the equipment slots. The camera only renders while the Inventory tab is
//! actually showing, so the preview costs nothing in normal play.
//!
//! The egui side reads the texture through [`paperdoll_preview_texture`], a write-once
//! `OnceLock` mirroring the item-icon registry (`app::ui::item_icons`): the
//! egui draw helpers are plain functions with no `World` access, so a global
//! avoids threading a resource through the paperdoll draw signature.

use std::collections::HashMap;
use std::sync::OnceLock;

use bevy::{
    camera::{ClearColorConfig, RenderTarget, visibility::RenderLayers},
    core_pipeline::tonemapping::Tonemapping,
    image::{BevyDefault, Image},
    light::NotShadowCaster,
    prelude::*,
    render::render_resource::TextureFormat,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

use crate::{
    items::{ArmorJoint, HeldMesh, item_definition},
    server::PlayerEquipmentVisual,
};

use super::super::scene::{PlayerPart, PlayerVisualAssets, rig_layout};
use super::super::state::{InventoryUiState, LocalPlayerState, MenuState, Screen};
use super::items::{
    ArmorVisuals, HeldItemVisuals, armor_layers, carry_forearm_rotation, carry_upper_arm_rotation,
    held_item_hand_transform, held_item_layers, insert_held_layer_material,
};

/// Render layer the preview rig, its light, and its camera live on. Layer 0 is
/// the world, layer 1 the first-person viewmodel; nothing else touches 2, so
/// the preview never leaks into the scene (and the scene's sun, gated to
/// layers {0,1}, never lights or shadows it).
const PAPERDOLL_RENDER_LAYER: usize = 2;

/// Off-screen target resolution. The inventory tab paints it at half this
/// size, so the preview supersamples 2x for clean edges without MSAA.
const PREVIEW_TEXTURE_WIDTH: u32 = 300;
const PREVIEW_TEXTURE_HEIGHT: u32 = 640;

/// Where the preview rig stands: far below the world so it can never be seen
/// by (or collide with) anything, even though the layer gate already isolates
/// it. The camera and light are positioned relative to this.
const PREVIEW_ORIGIN: Vec3 = Vec3::new(0.0, -900.0, 0.0);

/// Gentle idle sway applied to the rig's yaw so the preview reads as alive
/// without a full turntable spin.
const SWAY_AMPLITUDE_RAD: f32 = 0.20;
const SWAY_SPEED: f32 = 0.35;

/// Write-once egui texture id for the preview image, registered at startup by
/// [`setup_paperdoll_preview`]; `None` in headless/test contexts where the
/// setup never ran, so the paperdoll falls back to an empty frame.
static PREVIEW_TEXTURE: OnceLock<egui::TextureId> = OnceLock::new();

pub(crate) fn paperdoll_preview_texture() -> Option<egui::TextureId> {
    PREVIEW_TEXTURE.get().copied()
}

/// The preview's scene graph handles plus the edge-detection state for its
/// worn armor and held item (manual last-seen compare, the same pattern as the
/// remote-player rig).
#[derive(Resource)]
pub(crate) struct PaperdollPreview {
    camera: Entity,
    root: Entity,
    parts: HashMap<PlayerPart, Entity>,
    armor_layers: Vec<Entity>,
    last_equipment: PlayerEquipmentVisual,
    held_layers: Vec<Entity>,
    last_held: Option<HeldMesh>,
    /// Whether the carry pose is currently applied to the right arm (set when
    /// a held item is up so the tool sits in front of the body).
    carry_posed: bool,
}

/// Startup (after `setup_scene`, which builds the rig meshes): spawn the
/// preview rig + camera + lights and register the render target with egui.
pub(crate) fn setup_paperdoll_preview(
    mut commands: Commands,
    assets: Res<PlayerVisualAssets>,
    mut images: ResMut<Assets<Image>>,
    mut user_textures: ResMut<EguiUserTextures>,
) {
    let image = Image::new_target_texture(
        PREVIEW_TEXTURE_WIDTH,
        PREVIEW_TEXTURE_HEIGHT,
        TextureFormat::bevy_default(),
        None,
    );
    let image_handle = images.add(image);
    let texture_id = user_textures.add_image(EguiTextureHandle::Strong(image_handle.clone()));
    let _ = PREVIEW_TEXTURE.set(texture_id);

    let layer = RenderLayers::layer(PAPERDOLL_RENDER_LAYER);

    // The rig root; parts hang off it exactly like a remote player's rig.
    let root = commands
        .spawn((
            Name::new("Paperdoll Preview Rig"),
            Transform::from_translation(PREVIEW_ORIGIN),
            Visibility::default(),
        ))
        .id();
    let mut parts: HashMap<PlayerPart, Entity> = HashMap::new();
    for spec in rig_layout() {
        let parent = match spec.parent {
            Some(part) => parts[&part],
            None => root,
        };
        let mut entity = commands.spawn((
            spec.part,
            spec.rest,
            Visibility::Inherited,
            layer.clone(),
            NotShadowCaster,
            ChildOf(parent),
        ));
        if let Some(kind) = spec.mesh {
            entity.insert((
                Mesh3d(assets.rig.handle(kind)),
                MeshMaterial3d(assets.remote_material.clone()),
            ));
        }
        parts.insert(spec.part, entity.id());
    }

    // Head-on portrait camera. The rig's forward is -Z, so the camera sits on
    // the -Z side looking back at the figure; the idle sway turns the rig, not
    // the camera. Rendered before the main pass (negative order) so the image
    // is finished when egui samples it this frame.
    let camera = commands
        .spawn((
            Name::new("Paperdoll Preview Camera"),
            Camera3d::default(),
            Camera {
                order: -10,
                clear_color: ClearColorConfig::Custom(Color::NONE),
                is_active: false,
                ..default()
            },
            RenderTarget::Image(image_handle.into()),
            Tonemapping::AgX,
            Msaa::Off,
            Projection::from(PerspectiveProjection {
                // Sized so the 1.76m figure fills the portrait with a little
                // head- and foot-room at the 4.6m camera distance.
                fov: 26.0_f32.to_radians(),
                near: 0.1,
                far: 20.0,
                ..default()
            }),
            layer.clone(),
            Transform::from_translation(PREVIEW_ORIGIN + Vec3::new(0.0, 0.05, -4.6))
                .looking_at(PREVIEW_ORIGIN + Vec3::new(0.0, 0.02, 0.0), Vec3::Y),
        ))
        .id();

    // Fixed studio lighting, independent of the world's day/night cycle: a
    // warm key from the camera's upper left, a cool dim rim from behind the
    // figure's right shoulder, and a soft ambient fill on the camera so the
    // unlit side never crushes to black.
    commands.entity(camera).insert(AmbientLight {
        color: Color::srgb(0.85, 0.90, 1.0),
        brightness: 220.0,
        ..default()
    });
    commands.spawn((
        Name::new("Paperdoll Key Light"),
        DirectionalLight {
            color: Color::srgb(1.0, 0.96, 0.90),
            illuminance: 6500.0,
            shadows_enabled: false,
            ..default()
        },
        layer.clone(),
        Transform::default().looking_to(Vec3::new(0.35, -0.55, 0.75).normalize(), Vec3::Y),
    ));
    commands.spawn((
        Name::new("Paperdoll Rim Light"),
        DirectionalLight {
            color: Color::srgb(0.75, 0.85, 1.0),
            illuminance: 1800.0,
            shadows_enabled: false,
            ..default()
        },
        layer,
        Transform::default().looking_to(Vec3::new(-0.4, -0.25, -0.85).normalize(), Vec3::Y),
    ));

    commands.insert_resource(PaperdollPreview {
        camera,
        root,
        parts,
        armor_layers: Vec::new(),
        last_equipment: PlayerEquipmentVisual::default(),
        held_layers: Vec::new(),
        last_held: None,
        carry_posed: false,
    });
}

/// The rig joint entity(ies) an [`ArmorJoint`] attaches under, mirroring the
/// remote-player mapping in `players::armor_joint_entities` (helmets and chest
/// shells on the Body; symmetric aux/leg/feet shells fanned to both sides).
fn joint_entities(parts: &HashMap<PlayerPart, Entity>, joint: ArmorJoint) -> Vec<Entity> {
    use PlayerPart::*;
    match joint {
        ArmorJoint::Body => vec![parts[&Body]],
        ArmorJoint::UpperArmsBoth => vec![parts[&UpperArmL], parts[&UpperArmR]],
        ArmorJoint::ThighsBoth => vec![parts[&ThighL], parts[&ThighR]],
        ArmorJoint::ShinsBoth => vec![parts[&ShinL], parts[&ShinR]],
    }
}

/// Whether the preview should be rendering right now: the unified panel is on
/// the Inventory tab (not Admin, which replaces the paperdoll body) with no
/// pause covering it.
fn preview_visible(menu: &MenuState, inventory_ui: &InventoryUiState) -> bool {
    menu.screen == Screen::InGame
        && menu.inventory_open
        && !inventory_ui.admin_tab
        && !menu.pause_open
}

/// Per-frame sync: gate the camera to the Inventory tab, sway the rig, and
/// rebuild the worn-armor / held-item layers when the local (predicted)
/// inventory changes. Steady state with the tab open is two equality checks.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sync_paperdoll_preview_system(
    mut commands: Commands,
    preview: Option<ResMut<PaperdollPreview>>,
    menu: Res<MenuState>,
    inventory_ui: Res<InventoryUiState>,
    local_player: Res<LocalPlayerState>,
    armor_visuals: Res<ArmorVisuals>,
    held_visuals: Res<HeldItemVisuals>,
    time: Res<Time>,
    mut cameras: Query<&mut Camera>,
    mut transforms: Query<&mut Transform>,
) {
    let Some(mut preview) = preview else {
        return;
    };

    let active = preview_visible(&menu, &inventory_ui);
    if let Ok(mut camera) = cameras.get_mut(preview.camera)
        && camera.is_active != active
    {
        camera.is_active = active;
    }
    if !active {
        return;
    }

    // Gentle idle sway so the figure reads as alive.
    if let Ok(mut transform) = transforms.get_mut(preview.root) {
        let sway = (time.elapsed_secs() * SWAY_SPEED).sin() * SWAY_AMPLITUDE_RAD;
        transform.rotation = Quat::from_rotation_y(sway);
    }

    let inventory = local_player.private.as_ref().map(|p| &p.inventory);

    // Worn armor: the same derivation the server runs for peers, evaluated on
    // the local (predicted) inventory so a drag onto the paperdoll dresses the
    // preview the same frame.
    let equipment = inventory
        .map(|inventory| PlayerEquipmentVisual::from_equipment_slots(&inventory.equipment_slots))
        .unwrap_or_default();
    if equipment != preview.last_equipment {
        preview.last_equipment = equipment;
        for entity in std::mem::take(&mut preview.armor_layers) {
            commands.entity(entity).despawn();
        }
        let worn = [
            equipment.head,
            equipment.chest,
            equipment.legs,
            equipment.feet,
        ];
        for mesh in worn.into_iter().flatten() {
            for armor_layer in armor_layers(&armor_visuals, mesh) {
                for joint in joint_entities(&preview.parts, armor_layer.joint) {
                    let entity = commands
                        .spawn((
                            Name::new("Armor (paperdoll)"),
                            Mesh3d(armor_layer.mesh.clone()),
                            MeshMaterial3d(armor_layer.material.clone()),
                            Transform::IDENTITY,
                            Visibility::Inherited,
                            RenderLayers::layer(PAPERDOLL_RENDER_LAYER),
                            NotShadowCaster,
                            ChildOf(joint),
                        ))
                        .id();
                    preview.armor_layers.push(entity);
                }
            }
        }
    }

    // Held item: mirror the server's equipable filter so the preview matches
    // what peers (and the first-person viewmodel) show in hand.
    let held: Option<HeldMesh> = inventory
        .and_then(|inventory| inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .filter(|definition| definition.equipable)
        .map(|definition| definition.held_mesh);
    if held != preview.last_held {
        preview.last_held = held;
        for entity in std::mem::take(&mut preview.held_layers) {
            commands.entity(entity).despawn();
        }
        if let Some(mesh) = held {
            let grip = held_item_hand_transform(mesh);
            let anchor = preview.parts[&PlayerPart::HandAnchor];
            for held_layer in held_item_layers(&held_visuals, mesh, false) {
                let mut layer = commands.spawn((
                    Name::new("Held Item (paperdoll)"),
                    Mesh3d(held_layer.mesh),
                    grip,
                    Visibility::Inherited,
                    RenderLayers::layer(PAPERDOLL_RENDER_LAYER),
                    NotShadowCaster,
                    ChildOf(anchor),
                ));
                insert_held_layer_material(&mut layer, held_layer.material);
                preview.held_layers.push(layer.id());
            }
        }
        // Pose the right arm into the carry stance while something is held,
        // and relax it when the hand empties, so the tool sits in front of
        // the body exactly like the third-person rig.
        let carry = held.is_some();
        if carry != preview.carry_posed {
            preview.carry_posed = carry;
            let (upper, forearm) = if carry {
                (carry_upper_arm_rotation(), carry_forearm_rotation())
            } else {
                (Quat::IDENTITY, Quat::IDENTITY)
            };
            if let Ok(mut transform) = transforms.get_mut(preview.parts[&PlayerPart::UpperArmR]) {
                transform.rotation = upper;
            }
            if let Ok(mut transform) = transforms.get_mut(preview.parts[&PlayerPart::ForearmR]) {
                transform.rotation = forearm;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_only_renders_on_the_plain_inventory_tab() {
        let inventory_ui = InventoryUiState::default();
        // Closed panel: no preview.
        let mut menu = MenuState {
            screen: Screen::InGame,
            ..Default::default()
        };
        assert!(!preview_visible(&menu, &inventory_ui));

        // Inventory tab open in-game: preview renders.
        menu.inventory_open = true;
        assert!(preview_visible(&menu, &inventory_ui));

        // Pause covers the panel: preview stops.
        menu.pause_open = true;
        assert!(!preview_visible(&menu, &inventory_ui));
        menu.pause_open = false;

        // The Admin tab replaces the paperdoll body entirely.
        let mut admin_ui = InventoryUiState::default();
        admin_ui.admin_tab = true;
        assert!(!preview_visible(&menu, &admin_ui));

        // Not in-game (e.g. a menu screen): never.
        menu.screen = Screen::MainMenu;
        assert!(!preview_visible(&menu, &inventory_ui));
    }
}
