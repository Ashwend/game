//! Live render-entity counts for the F2 perf overlay: how many `Mesh3d` entities
//! exist versus how many actually survive visibility culling (are drawn in the
//! main view or a shadow cascade). It answers "are we rendering the whole world
//! regardless of where the camera looks?" at a glance, which the server-derived
//! `Visible` node count (AoI ring, view-independent) cannot.
//!
//! Routed through Bevy diagnostics so the perf overlay reads it the same way it
//! reads FPS/frame-time, no extra plumbing through the UI param bundle. Measured
//! only while the overlay is up, so it costs nothing in normal play.

use bevy::{
    camera::visibility::ViewVisibility,
    diagnostic::{Diagnostic, DiagnosticPath, Diagnostics, RegisterDiagnostic},
    prelude::*,
};

use crate::app::state::ClientSettings;

/// Total `Mesh3d` entities in the client world.
pub(crate) const MESH_TOTAL: DiagnosticPath = DiagnosticPath::const_new("render/mesh_total");
/// `Mesh3d` entities that passed visibility culling this frame (visible to at
/// least one view, the main camera OR a sun shadow cascade). If this stays near
/// [`MESH_TOTAL`] no matter which way the camera faces, the scene is being drawn
/// wholesale rather than culled to the view, which is itself a finding.
pub(crate) const MESH_VISIBLE: DiagnosticPath = DiagnosticPath::const_new("render/mesh_visible");

/// Registers the two diagnostics and the measuring system on the client app.
pub(crate) fn register_render_stats(app: &mut App) {
    app.register_diagnostic(Diagnostic::new(MESH_TOTAL))
        .register_diagnostic(Diagnostic::new(MESH_VISIBLE))
        .add_systems(Update, measure_render_stats_system);
}

/// Counts the `Mesh3d` set only while the perf overlay (F2) is on, so it is free
/// otherwise. `ViewVisibility` is written by Bevy's `check_visibility` in
/// `PostUpdate`; reading it here in `Update` uses last frame's value, which is
/// exactly right for a diagnostic readout.
fn measure_render_stats_system(
    settings: Res<ClientSettings>,
    mut diagnostics: Diagnostics,
    meshes: Query<&ViewVisibility, With<Mesh3d>>,
) {
    if !settings.hud.show_perf_stats {
        return;
    }
    let mut total: u32 = 0;
    let mut visible: u32 = 0;
    for view_visibility in &meshes {
        total += 1;
        if view_visibility.get() {
            visible += 1;
        }
    }
    diagnostics.add_measurement(&MESH_TOTAL, || f64::from(total));
    diagnostics.add_measurement(&MESH_VISIBLE, || f64::from(visible));
}
