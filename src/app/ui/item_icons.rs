//! Inventory item icon textures.
//!
//! Each registered item ships a transparent PNG at
//! `assets/items/<id>/icon.png`, baked into the binary by the embedded-asset
//! tree and loaded here at startup. [`setup_item_icons`] registers every icon
//! with `bevy_egui` and caches the resulting [`egui::TextureId`]s; the
//! inventory renderer (`super::inventory::slot::paint_item_icon`) looks them
//! up by item id and falls back to a tinted-rectangle placeholder when an icon
//! is missing.
//!
//! The id -> texture map is process-global and write-once (the icons never
//! change after startup), so an [`OnceLock`] is the right shape rather than a
//! `Resource`: the egui draw helpers are plain functions with no `World`
//! access, and a `OnceLock` lets them read the map without threading a resource
//! through every slot/modal/drag draw signature. This mirrors the write-once
//! registry globals in [`crate::items`]. The strong `Handle<Image>`s stay alive
//! for the life of the app inside `bevy_egui`'s `EguiUserTextures` (which holds
//! the strong handle we pass to `add_image`), so we don't keep a second copy.

use std::{collections::HashMap, sync::OnceLock};

use bevy::prelude::*;
use bevy_egui::{EguiTextureHandle, EguiUserTextures, egui};

use crate::{
    app::embedded_asset_path,
    items::{DeployableKind, REGISTERED_ITEMS},
};

/// Write-once `item id -> egui texture id` map, populated by
/// [`setup_item_icons`] at startup and read by the inventory renderer.
static ICON_TEXTURES: OnceLock<HashMap<&'static str, egui::TextureId>> = OnceLock::new();

/// The egui texture id for an item's inventory icon, if one was registered at
/// startup. Returns `None` in headless/test contexts where [`setup_item_icons`]
/// never ran, or for an item with no shipped icon, so callers fall back to the
/// placeholder.
pub(crate) fn texture_for(item_id: &str) -> Option<egui::TextureId> {
    ICON_TEXTURES.get()?.get(item_id).copied()
}

/// Startup system: load every registered item's
/// `embedded://items/<id>/icon.png`, register it with `bevy_egui`, and cache the
/// texture ids for the inventory renderer. The texture is uploaded to the GPU
/// once its image asset finishes loading; icons are small and loaded at startup,
/// so they are ready well before the player opens any inventory surface.
pub(crate) fn setup_item_icons(
    asset_server: Res<AssetServer>,
    mut user_textures: ResMut<EguiUserTextures>,
) {
    let mut map = HashMap::with_capacity(REGISTERED_ITEMS.len());
    for definition in REGISTERED_ITEMS {
        // Building blocks are hidden registry entries (placed via the
        // building plan, never held in an inventory), so they ship no
        // icon; skip them rather than logging a missing-asset error.
        if matches!(
            definition.deployable.map(|profile| profile.kind),
            Some(DeployableKind::Building { .. })
        ) {
            continue;
        }
        let path = embedded_asset_path(&format!("items/{}/icon.png", definition.id));
        let handle: Handle<Image> = asset_server.load(path);
        let texture_id = user_textures.add_image(EguiTextureHandle::Strong(handle));
        map.insert(definition.id, texture_id);
    }
    // Write-once: a second App in the same process (unusual outside tests)
    // keeps the first run's ids rather than overwriting mid-flight.
    let _ = ICON_TEXTURES.set(map);
}
