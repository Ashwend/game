//! Pushes the debug-only `Dev` options tab toggles into live shader state. Built
//! only on debug builds (the tab is hidden in release and the `dev_flags` uniforms
//! stay 0 there, so this carries zero shipped cost).
//!
//! The toon toggles ride a `dev_flags` uniform on every cached toon material; the
//! grass toggles ride a small render-world uniform updated from [`GrassDevFlags`].
//! Pipeline toggles (bloom / shadows / atmosphere / fog) are folded into
//! `apply_graphics_settings_system` and `update_sky_system` instead, since those
//! already own the components.

use bevy::prelude::*;

use crate::app::{
    scene::{GrassDevFlags, ToonMaterial, ToonViewmodelMaterial},
    state::ClientSettings,
};

/// Write the `Dev` tab's shader toggles into the toon materials' `dev_flags`
/// uniform and the grass dev-flags resource. Change-gated on `ClientSettings`, so
/// it also runs on the first frame (a persisted non-default panel state takes
/// effect at boot). The toon materials are a small fixed set of cached handles, so
/// touching all of them on a (rare) settings change is cheap.
pub(crate) fn apply_dev_render_settings(
    settings: Res<ClientSettings>,
    mut toon: ResMut<Assets<ToonMaterial>>,
    mut toon_vm: ResMut<Assets<ToonViewmodelMaterial>>,
    mut grass: ResMut<GrassDevFlags>,
) {
    if !settings.is_changed() {
        return;
    }
    let flags = settings.dev.toon_flags();
    for (_, material) in toon.iter_mut() {
        material.dev_flags = flags;
    }
    for (_, material) in toon_vm.iter_mut() {
        material.dev_flags = flags;
    }
    let grass_flags = settings.dev.grass_flags();
    if grass.0 != grass_flags {
        grass.0 = grass_flags;
    }
}
