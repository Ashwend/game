//! Worn-armor rig visuals: the [`ArmorVisuals`] lookup resource and the
//! `build_armor_visuals` fold that turns the declarative [`ArmorMesh::visual`]
//! table (src/items/visual.rs) into ready `(Handle<Mesh>, material, joint)`
//! layers, the armor analogue of [`super::held::HeldItemVisuals`] and
//! `build_held_item_visuals`.
//!
//! Armor matches the player rig's material family, PBR `StandardMaterial` (not
//! the cel/toon family the held tools use), so each [`ArmorMaterial`] family
//! resolves to one `StandardMaterial` handle. The three family handles are built
//! once in `setup_scene` from the detail textures and handed to
//! [`build_armor_visuals`] as an [`ArmorMaterials`] set. COLOR_0 on the glb
//! carries identity; the texture only adds surface grain, exactly how the rig
//! itself renders.
//!
//! The rig-attachment system that consumes this resource lives in
//! `app::systems::players` next to the `PlayerRig` and `RemoteEquipment` mirror
//! it drives.

use std::collections::HashMap;

use bevy::{gltf::GltfAssetLabel, prelude::*};

use crate::{
    app::embedded_asset_path,
    items::{ArmorJoint, ArmorMaterial, ArmorMesh},
};

/// The three shared PBR `StandardMaterial` handles armor binds, one per
/// [`ArmorMaterial`] family. Built once in `setup_scene` from the detail
/// textures (cloth, wood slat, steel) and passed to [`build_armor_visuals`], so
/// the whole armor catalogue costs exactly three materials.
#[derive(Clone)]
pub(crate) struct ArmorMaterials {
    pub(crate) cloth: Handle<StandardMaterial>,
    pub(crate) wood_slat: Handle<StandardMaterial>,
    pub(crate) steel: Handle<StandardMaterial>,
}

impl ArmorMaterials {
    fn resolve(&self, family: ArmorMaterial) -> Handle<StandardMaterial> {
        match family {
            ArmorMaterial::Cloth => self.cloth.clone(),
            ArmorMaterial::WoodSlat => self.wood_slat.clone(),
            ArmorMaterial::Steel => self.steel.clone(),
        }
    }
}

/// One attached layer of a worn-armor piece, resolved to concrete handles: the
/// glb-primitive mesh, its PBR material, and the rig joint it parents under.
/// A chest piece has two layers (a torso shell on the body, a shoulder aux on
/// both upper arms); every other piece has one.
#[derive(Clone)]
pub(crate) struct ArmorLayer {
    pub(crate) mesh: Handle<Mesh>,
    pub(crate) material: Handle<StandardMaterial>,
    pub(crate) joint: ArmorJoint,
}

/// Precomputed rig layers for every [`ArmorMesh`], built once from the
/// declarative [`ArmorMesh::visual`] table in `setup_scene` (see
/// [`build_armor_visuals`]), then read by a plain map lookup in
/// [`armor_layers`] when the rig-attachment system rebuilds a player's worn
/// armor on an equipment change. World-lit only (armor rides on remote players
/// in world space, never on the first-person viewmodel camera).
#[derive(Resource)]
pub(crate) struct ArmorVisuals {
    layers: HashMap<ArmorMesh, Vec<ArmorLayer>>,
}

impl ArmorVisuals {
    /// The attached layers for a worn `mesh`, or an empty vec if the mesh has no
    /// row (which the completeness test forbids). Cloned so the caller owns the
    /// handles it spawns.
    pub(crate) fn layers(&self, mesh: ArmorMesh) -> Vec<ArmorLayer> {
        self.layers.get(&mesh).cloned().unwrap_or_default()
    }
}

/// Look up the rig layers for a worn armor `mesh`. A plain map lookup into the
/// precomputed [`ArmorVisuals`] resource; kept as a free function to mirror
/// [`super::held::held_item_layers`].
pub(crate) fn armor_layers(visuals: &ArmorVisuals, mesh: ArmorMesh) -> Vec<ArmorLayer> {
    visuals.layers(mesh)
}

/// Fold the declarative [`ArmorMesh::visual`] table into the [`ArmorVisuals`]
/// lookup, loading each layer's glb primitive and resolving its material family
/// through the shared [`ArmorMaterials`] set built in `setup_scene`. Called once
/// from `setup_scene`.
///
/// Loads each glb primitive at most once (per mesh + primitive index) and shares
/// the three material handles across every piece of a family, so the whole armor
/// catalogue costs three materials plus one mesh handle per distinct primitive.
pub(crate) fn build_armor_visuals(
    asset_server: &AssetServer,
    armor_materials: &ArmorMaterials,
) -> ArmorVisuals {
    let prim_mesh = |glb: &str, primitive: usize| -> Handle<Mesh> {
        asset_server.load(
            GltfAssetLabel::Primitive { mesh: 0, primitive }.from_asset(embedded_asset_path(glb)),
        )
    };

    let mut layers = HashMap::new();
    for &armor_mesh in ArmorMesh::ALL {
        let visual = armor_mesh.visual();
        let piece_layers: Vec<ArmorLayer> = visual
            .layers()
            .map(|spec| ArmorLayer {
                mesh: prim_mesh(visual.glb, spec.primitive),
                material: armor_materials.resolve(spec.material),
                joint: spec.joint,
            })
            .collect();
        layers.insert(armor_mesh, piece_layers);
    }
    ArmorVisuals { layers }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every [`ArmorMesh`] resolves each of its material families to one of the
    /// three PBR families, so a piece can never reach the builder with a family
    /// the resolver does not know. Pure: no app or GPU needed.
    #[test]
    fn every_armor_layer_binds_a_known_family() {
        for &mesh in ArmorMesh::ALL {
            for layer in mesh.visual().layers() {
                assert!(
                    matches!(
                        layer.material,
                        ArmorMaterial::Cloth | ArmorMaterial::WoodSlat | ArmorMaterial::Steel
                    ),
                    "{mesh:?} layer binds an unexpected material family {:?}",
                    layer.material
                );
            }
        }
    }
}
