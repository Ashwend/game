//! World-map client systems: the toggle input (which also fires the throttled
//! marker fetch) and the local generation of the biome terrain texture.
//!
//! The map is a *toggle* overlay, not hold-to-view: press the key to open it,
//! press it (or Escape) to close. It frees the cursor so the player can drop
//! and label markers on it, and movement stays live so they can run with the
//! map up to check their coordinates (look/swing stay frozen). The terrain
//! image is generated client-side from the world seed (a pure function of it,
//! see [`crate::world::map_texture`]); only the per-account markers come from
//! the server. The overlay itself is drawn in [`crate::app::ui`]; these systems
//! manage the open/closed flag, the marker request cadence, and the texture.
//! See [`crate::app::state::WorldMapState`].

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    window::PrimaryWindow,
};
use bevy_egui::{EguiTextureHandle, EguiUserTextures};

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, KeyAction, MenuState, Screen,
        WorldMapState, WorldMapUiState,
    },
    protocol::{ClientMessage, WorldMapMarkerCommand},
    world::{WORLD_MAP_TEXELS, render_world_map_rgba, world_map_bounds},
};

/// Toggle the world map with its key. Opening fires a throttled
/// [`ClientMessage::RequestWorldMap`] when the cached map is stale. The flag
/// this sets (`menu.world_map_open`) frees the cursor and gates look/swing via
/// `gameplay_accepts_controls`, never the simulation; movement stays live
/// through `gameplay_accepts_movement`.
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn world_map_input_system(
    time: Res<Time>,
    settings: Res<ClientSettings>,
    keys: Res<ButtonInput<KeyCode>>,
    primary_window: Query<&Window, With<PrimaryWindow>>,
    mut menu: ResMut<MenuState>,
    mut runtime: ResMut<ClientRuntime>,
    mut world_map: ResMut<WorldMapState>,
    mut world_map_ui: ResMut<WorldMapUiState>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
) {
    if menu.screen != Screen::InGame {
        if menu.world_map_open {
            menu.world_map_open = false;
        }
        if world_map_ui.is_dirty() {
            world_map_ui.clear();
        }
        return;
    }

    // Drain a confirmed marker deletion (armed by the confirm modal, which has
    // no session access) and send the server the remove command.
    if let Some(id) = menu.world_map_delete_pending.take()
        && let Some(session) = runtime.session.as_mut()
        && let Err(error) = session.send(ClientMessage::WorldMapMarker(
            WorldMapMarkerCommand::Remove { id },
        ))
    {
        error_toasts.write(ClientErrorToast::new(format!(
            "couldn't delete marker: {error}"
        )));
    }

    // Whenever the map is closed (incl. closed by Escape elsewhere), reset the
    // popup selection and the pan/zoom viewport so a later open starts fresh.
    if !menu.world_map_open && world_map_ui.is_dirty() {
        world_map_ui.clear();
    }

    let focused = primary_window
        .single()
        .map(|window| window.focused)
        .unwrap_or(true);

    // Modals that should never coexist with the map. The marker-name text
    // prompt is excluded: naming a marker opens one on top of the still-open
    // map on purpose.
    let blocking_modal = menu.pause_open
        || menu.inventory_open
        || menu.crafting_open
        || menu.furnace_open
        || menu.loot_bag_open
        || menu.chat_open
        || menu.death_splash.is_some();

    // A blocking modal appearing over the map closes it: the map can't stay
    // interactive behind another panel. Losing window focus deliberately does
    // NOT close it (controls already freeze on unfocus); the map stays put so
    // alt-tabbing to glance at something doesn't discard it.
    if menu.world_map_open && blocking_modal {
        menu.world_map_open = false;
        world_map_ui.clear();
        return;
    }

    // While a dialog owns the screen (marker-name prompt, or the delete
    // confirm), the map key is inert so a typed "m" can't slam the map shut out
    // from under the dialog, and Enter/Escape stay with the dialog.
    if menu.dialog_modal_open() {
        return;
    }

    if !(focused
        && settings
            .keybindings
            .just_pressed(KeyAction::WorldMap, &keys))
    {
        return;
    }
    // Don't open the map on top of another modal (but a closing press always
    // works).
    if !menu.world_map_open && blocking_modal {
        return;
    }

    menu.world_map_open = !menu.world_map_open;
    if menu.world_map_open {
        // Opening edge: fetch a fresh map if the cache went stale and nothing
        // is already in flight.
        if world_map.should_request(time.elapsed_secs())
            && let Some(session) = runtime.session.as_mut()
            && session.send(ClientMessage::RequestWorldMap).is_ok()
        {
            world_map.mark_requested(time.elapsed_secs());
        }
    } else {
        world_map_ui.clear();
    }
}

/// Generate the biome terrain texture locally the first time the map is opened.
/// The raster is a pure function of the world seed (from `Welcome`), so this
/// runs once per world: 65k cheap noise samples on the open frame, then the
/// texture is cached and this early-returns. No server round trip for the image.
pub(crate) fn generate_world_map_texture_system(
    menu: Res<MenuState>,
    runtime: Res<ClientRuntime>,
    mut world_map: ResMut<WorldMapState>,
    mut images: ResMut<Assets<Image>>,
    mut user_textures: ResMut<EguiUserTextures>,
) {
    // Only when the map is open and we don't already have the texture.
    if !menu.world_map_open || world_map.texture().is_some() {
        return;
    }
    let Some((seed, dims)) = runtime.world_map_seed_dims else {
        return;
    };

    let rgba = render_world_map_rgba(seed, dims);
    let bounds = world_map_bounds(dims);
    let image = make_map_image(WORLD_MAP_TEXELS, WORLD_MAP_TEXELS, rgba);
    let handle = images.add(image);
    let texture = user_textures.add_image(EguiTextureHandle::Strong(handle.clone()));
    world_map.set_texture(texture, handle, bounds);
}

/// Build a linearly-sampled RGBA image from raw bytes. Linear sampling is what
/// turns the low-res biome raster into soft, cartoonish blobs on upscale.
fn make_map_image(width: u32, height: u32, rgba: Vec<u8>) -> Image {
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    image
}
