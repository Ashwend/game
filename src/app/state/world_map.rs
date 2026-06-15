//! Client-side world-map state.
//!
//! Holds the locally-generated biome terrain texture, its world bounds, the
//! player's own markers, and the marker fetch/throttle bookkeeping. The terrain
//! image is rendered on the client from the world seed (see
//! [`crate::world::map_texture`]), not received from the server; only the
//! per-account markers come over the wire. The open/closed flag itself lives on
//! [`super::MenuState`] (`world_map_open`) because it gates controls. A separate
//! [`WorldMapUiState`] holds the transient marker-popup + pan/zoom state.
//! Filled by the systems in [`crate::app::systems`]; read (and the popup state
//! mutated) by the overlay in [`crate::app::ui`].

use bevy::prelude::*;
use bevy_egui::egui;

use crate::protocol::WorldMapMarker;

/// How long fetched markers stay fresh before a re-open re-requests them. The
/// server pushes updates on every edit, so this only matters for picking up
/// markers placed in a previous session after a reconnect; a slow cadence keeps
/// that working without spamming the server.
pub(crate) const WORLD_MAP_CACHE_SECONDS: f32 = 60.0;

/// If a marker request goes unanswered this long, a re-open may retry it.
/// Covers a dropped reliable message or a map opened mid-handshake before the
/// session could send.
pub(crate) const WORLD_MAP_REQUEST_TIMEOUT_SECONDS: f32 = 8.0;

/// Client-side world-map data. See the module docs.
#[derive(Resource, Default)]
pub(crate) struct WorldMapState {
    /// egui handle for the locally-generated biome image, once it exists.
    texture: Option<egui::TextureId>,
    /// Strong handle keeping the generated image alive.
    image: Option<Handle<Image>>,
    /// World-space AABB the texture covers: `(min_x, min_z, max_x, max_z)`.
    bounds: Option<(f32, f32, f32, f32)>,
    /// The player's own hand-placed markers, in world coordinates.
    markers: Vec<WorldMapMarker>,
    /// `Time::elapsed_secs()` when markers last landed (drives the cache TTL).
    fetched_at: Option<f32>,
    /// `Time::elapsed_secs()` when the in-flight marker request was sent (drives
    /// the retry timeout and suppresses duplicate requests).
    requested_at: Option<f32>,
}

impl WorldMapState {
    pub(crate) fn texture(&self) -> Option<egui::TextureId> {
        self.texture
    }

    pub(crate) fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        self.bounds
    }

    pub(crate) fn markers(&self) -> &[WorldMapMarker] {
        &self.markers
    }

    /// Record the locally-generated terrain texture + its world bounds. Done
    /// once per world (the raster is a pure function of the seed).
    pub(crate) fn set_texture(
        &mut self,
        texture: egui::TextureId,
        image: Handle<Image>,
        bounds: (f32, f32, f32, f32),
    ) {
        self.texture = Some(texture);
        self.image = Some(image);
        self.bounds = Some(bounds);
    }

    /// Whether opening the map now should fire a (re)request for markers: none
    /// fetched yet, or the cache went stale, and nothing is already in flight.
    pub(crate) fn should_request(&self, now: f32) -> bool {
        let stale = self
            .fetched_at
            .is_none_or(|fetched| now - fetched > WORLD_MAP_CACHE_SECONDS);
        let in_flight = self
            .requested_at
            .is_some_and(|sent| now - sent < WORLD_MAP_REQUEST_TIMEOUT_SECONDS);
        stale && !in_flight
    }

    /// Record that a marker request was just sent, so a held-open map doesn't
    /// keep firing requests every frame.
    pub(crate) fn mark_requested(&mut self, now: f32) {
        self.requested_at = Some(now);
    }

    /// Replace the marker list (server reply on open, or push after an edit)
    /// and refresh the cache timestamp.
    pub(crate) fn apply_markers(&mut self, markers: Vec<WorldMapMarker>, now: f32) {
        self.markers = markers;
        self.fetched_at = Some(now);
        self.requested_at = None;
    }
}

/// Transient world-map interaction state: the marker popup selection plus the
/// pan/zoom viewport. Separate from [`WorldMapState`] (which is the replicated
/// data the overlay draws) because this is local UI bookkeeping the overlay
/// both reads and writes each frame.
#[derive(Resource)]
pub(crate) struct WorldMapUiState {
    /// The id of the marker whose name/delete popup is showing, if any.
    pub(crate) selected_marker: Option<u32>,
    /// Zoom factor: 1.0 fits the whole world in the map square; higher zooms
    /// in. Clamped by the overlay.
    pub(crate) zoom: f32,
    /// World `(x, z)` shown at the centre of the map square. `None` means
    /// "world centre" (the only valid centre at zoom 1); set once the player
    /// pans or zooms.
    pub(crate) center: Option<(f32, f32)>,
}

impl Default for WorldMapUiState {
    fn default() -> Self {
        Self {
            selected_marker: None,
            zoom: 1.0,
            center: None,
        }
    }
}

impl WorldMapUiState {
    /// Reset the overlay's transient state (popup selection + viewport). Called
    /// when the map closes so a later open starts fresh: no stale popup, zoomed
    /// all the way out, centred on the world.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether anything is in a non-default state, used to decide if a closed
    /// map needs resetting.
    pub(crate) fn is_dirty(&self) -> bool {
        self.selected_marker.is_some() || self.center.is_some() || self.zoom != 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_open_requests_then_waits_for_the_reply() {
        let mut state = WorldMapState::default();
        // Nothing fetched yet: should request.
        assert!(state.should_request(0.0));
        state.mark_requested(0.0);
        // In flight: don't double-request.
        assert!(!state.should_request(1.0));
        // Request timed out with no reply: allowed to retry.
        assert!(state.should_request(WORLD_MAP_REQUEST_TIMEOUT_SECONDS + 0.1));
    }

    #[test]
    fn fresh_cache_suppresses_requests_until_it_goes_stale() {
        let mut state = WorldMapState::default();
        state.apply_markers(Vec::new(), 100.0);
        assert!(!state.should_request(100.0));
        assert!(!state.should_request(100.0 + WORLD_MAP_CACHE_SECONDS - 1.0));
        assert!(state.should_request(100.0 + WORLD_MAP_CACHE_SECONDS + 1.0));
    }
}
