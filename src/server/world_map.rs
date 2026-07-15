//! Server-side world-map markers.
//!
//! The biome *terrain* image isn't here, it's a pure function of the seed the
//! client already has, so the client renders it locally (see
//! [`crate::world::map_texture`]). This module only owns the per-player marker
//! store: the points the player drops by hand, kept per-account so a shared map
//! never reveals another player's pins, and persisted in the world save.

use std::collections::HashMap;

use crate::{
    game_balance::{WORLD_MAP_MARKER_MAX_PER_PLAYER, WORLD_MAP_MARKER_NAME_MAX_LEN},
    protocol::{AccountId, ClientId, ServerMessage, WorldMapMarker, WorldMapMarkerCommand},
    save::PersistedAccountMarkers,
};

use super::{DeliveryTarget, GameServer, ServerEnvelope};

/// Server-authoritative store of every player's hand-placed map markers,
/// keyed by account. Lives on [`GameServer`] and is persisted to the world
/// save. Markers are private: a request only ever sees the asker's own list.
#[derive(Debug, Default, Clone)]
pub(crate) struct WorldMapMarkerStore {
    by_account: HashMap<AccountId, Vec<WorldMapMarker>>,
    /// Monotonic id, unique across every account on this world. Re-derived on
    /// load from the highest stored id so it never collides with a survivor.
    next_id: u32,
}

impl WorldMapMarkerStore {
    /// The caller's own markers, in insertion order. Empty slice when the
    /// account has never placed one.
    pub(crate) fn markers_for(&self, account: AccountId) -> &[WorldMapMarker] {
        self.by_account
            .get(&account)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Drop a new unnamed marker. Returns `false` (and adds nothing) once the
    /// account is at the per-player cap, so a client can't bloat the save.
    fn add(&mut self, account: AccountId, x: f32, z: f32) -> bool {
        let list = self.by_account.entry(account).or_default();
        if list.len() >= WORLD_MAP_MARKER_MAX_PER_PLAYER {
            return false;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        list.push(WorldMapMarker {
            id,
            x,
            z,
            name: String::new(),
        });
        true
    }

    /// Set (or clear, when the sanitized name is empty) a marker's label.
    fn rename(&mut self, account: AccountId, id: u32, name: String) {
        if let Some(marker) = self
            .by_account
            .get_mut(&account)
            .and_then(|list| list.iter_mut().find(|marker| marker.id == id))
        {
            marker.name = name;
        }
    }

    /// Delete a marker the account owns. No-op for an unknown id.
    fn remove(&mut self, account: AccountId, id: u32) {
        if let Some(list) = self.by_account.get_mut(&account) {
            list.retain(|marker| marker.id != id);
        }
    }

    /// Snapshot for the world save, sorted by account for deterministic bytes.
    pub(crate) fn to_persisted(&self) -> Vec<PersistedAccountMarkers> {
        let mut out: Vec<PersistedAccountMarkers> = self
            .by_account
            .iter()
            .filter(|(_, markers)| !markers.is_empty())
            .map(|(account_id, markers)| PersistedAccountMarkers {
                account_id: *account_id,
                markers: markers.clone(),
            })
            .collect();
        out.sort_by_key(|entry| entry.account_id);
        out
    }

    /// Rebuild from a loaded save, flooring `next_id` above the highest stored
    /// id so a future add can't reuse a survivor's handle.
    pub(crate) fn from_persisted(persisted: Vec<PersistedAccountMarkers>) -> Self {
        let mut by_account = HashMap::new();
        let mut highest_id = None;
        for entry in persisted {
            for marker in &entry.markers {
                highest_id = Some(highest_id.map_or(marker.id, |h: u32| h.max(marker.id)));
            }
            by_account.insert(entry.account_id, entry.markers);
        }
        let next_id = highest_id.map_or(0, |h| h.wrapping_add(1));
        Self {
            by_account,
            next_id,
        }
    }
}

/// Trim a marker label to the accepted length, dropping control characters so
/// a client can't smuggle newlines or escape codes into the persisted name.
fn sanitize_marker_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(WORLD_MAP_MARKER_NAME_MAX_LEN)
        .collect()
}

impl GameServer {
    /// Answer a [`crate::protocol::ClientMessage::RequestWorldMap`]: return the
    /// caller's own markers (the terrain image is generated client-side).
    pub(super) fn apply_world_map_request(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(account_id) = self.clients.get(&client_id).map(|client| client.account_id) else {
            return Vec::new();
        };

        // Only the asker's own markers: a shared map never reveals another
        // player's pins.
        let markers = self.world_map_markers.markers_for(account_id).to_vec();

        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::WorldMapMarkers { markers },
        }]
    }

    /// Apply an add / rename / remove from
    /// [`crate::protocol::ClientMessage::WorldMapMarker`] and reply with the
    /// caller's full updated list so the overlay refreshes immediately.
    pub(super) fn apply_world_map_marker_command(
        &mut self,
        client_id: ClientId,
        command: WorldMapMarkerCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(account_id) = self.clients.get(&client_id).map(|client| client.account_id) else {
            return Vec::new();
        };

        match command {
            WorldMapMarkerCommand::Add { x, z } => {
                // Reject non-finite coordinates: a NaN pin would render
                // nowhere and poison the bounds math on the client.
                if x.is_finite() && z.is_finite() {
                    self.world_map_markers.add(account_id, x, z);
                }
            }
            WorldMapMarkerCommand::Rename { id, name } => {
                self.world_map_markers
                    .rename(account_id, id, sanitize_marker_name(&name));
            }
            WorldMapMarkerCommand::Remove { id } => {
                self.world_map_markers.remove(account_id, id);
            }
        }

        let markers = self.world_map_markers.markers_for(account_id).to_vec();
        vec![ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::WorldMapMarkers { markers },
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the marker list out of a `WorldMapMarkers` reply envelope.
    fn reply_markers(envelopes: &[ServerEnvelope]) -> &[WorldMapMarker] {
        assert_eq!(envelopes.len(), 1, "one reply to the caller");
        match &envelopes[0].message {
            ServerMessage::WorldMapMarkers { markers } => markers,
            other => panic!("expected a WorldMapMarkers reply, got {other:?}"),
        }
    }

    #[test]
    fn add_rename_remove_round_trips_through_the_command() {
        let mut server = crate::server::test_support::server();
        let me = crate::server::test_support::connect_named(&mut server, "Me");

        // Add: the reply echoes the new pin with a server-assigned id.
        let reply = server
            .apply_world_map_marker_command(me, WorldMapMarkerCommand::Add { x: 12.0, z: -8.0 });
        let markers = reply_markers(&reply);
        assert_eq!(markers.len(), 1);
        assert!(markers[0].name.is_empty(), "a fresh marker is unnamed");
        let id = markers[0].id;
        assert!((markers[0].x - 12.0).abs() < 0.001);

        // Rename: the label sticks.
        let reply = server.apply_world_map_marker_command(
            me,
            WorldMapMarkerCommand::Rename {
                id,
                name: "Base".to_owned(),
            },
        );
        assert_eq!(reply_markers(&reply)[0].name, "Base");

        // Remove: the list empties.
        let reply = server.apply_world_map_marker_command(me, WorldMapMarkerCommand::Remove { id });
        assert!(reply_markers(&reply).is_empty());
    }

    #[test]
    fn request_returns_only_the_callers_own_markers() {
        let mut server = crate::server::test_support::server();
        // `connect_named` pins everyone to account id 1, so a second connect
        // would just reconnect the same account. Add the caller's pins through
        // the command, then seed a *different* account's pins straight into the
        // store to stand in for another player.
        let me = crate::server::test_support::connect_named(&mut server, "Me");
        let my_account = server.clients[&me].account_id;

        server.apply_world_map_marker_command(me, WorldMapMarkerCommand::Add { x: 10.0, z: 20.0 });
        server.apply_world_map_marker_command(me, WorldMapMarkerCommand::Add { x: 5.0, z: 6.0 });
        server
            .world_map_markers
            .add(crate::protocol::AccountId(my_account.0 + 1), 30.0, 30.0);

        let reply = server.apply_world_map_request(me);
        let markers = reply_markers(&reply);
        assert_eq!(markers.len(), 2, "only the caller's own markers");
        assert!(
            markers.iter().all(|marker| (marker.x - 30.0).abs() > 0.5),
            "the other account's pin must be excluded"
        );
    }

    #[test]
    fn add_rejects_non_finite_coordinates() {
        let mut server = crate::server::test_support::server();
        let me = crate::server::test_support::connect_named(&mut server, "Me");
        let reply = server.apply_world_map_marker_command(
            me,
            WorldMapMarkerCommand::Add {
                x: f32::NAN,
                z: 0.0,
            },
        );
        assert!(
            reply_markers(&reply).is_empty(),
            "a NaN coordinate must not create a pin"
        );
    }

    #[test]
    fn rename_sanitizes_and_truncates_the_label() {
        let cleaned = sanitize_marker_name("  hi\nthere\t!  ");
        assert_eq!(cleaned, "hithere!", "control chars dropped, ends trimmed");

        let long: String = "x".repeat(WORLD_MAP_MARKER_NAME_MAX_LEN + 20);
        assert_eq!(
            sanitize_marker_name(&long).chars().count(),
            WORLD_MAP_MARKER_NAME_MAX_LEN
        );
    }

    #[test]
    fn add_stops_at_the_per_player_cap() {
        let mut store = WorldMapMarkerStore::default();
        for _ in 0..WORLD_MAP_MARKER_MAX_PER_PLAYER {
            assert!(store.add(crate::protocol::AccountId(1), 0.0, 0.0));
        }
        assert!(
            !store.add(crate::protocol::AccountId(1), 0.0, 0.0),
            "the cap must reject the next add and leave the list unchanged"
        );
        assert_eq!(
            store.markers_for(crate::protocol::AccountId(1)).len(),
            WORLD_MAP_MARKER_MAX_PER_PLAYER
        );
    }

    #[test]
    fn persistence_round_trips_and_floors_the_id_counter() {
        let mut store = WorldMapMarkerStore::default();
        store.add(crate::protocol::AccountId(1), 1.0, 2.0);
        store.add(crate::protocol::AccountId(1), 3.0, 4.0);
        store.add(crate::protocol::AccountId(2), 5.0, 6.0);

        let restored = WorldMapMarkerStore::from_persisted(store.to_persisted());
        assert_eq!(restored.markers_for(crate::protocol::AccountId(1)).len(), 2);
        assert_eq!(restored.markers_for(crate::protocol::AccountId(2)).len(), 1);

        // A new add must not reuse a surviving id.
        let mut restored = restored;
        let existing: std::collections::HashSet<u32> = restored
            .markers_for(crate::protocol::AccountId(1))
            .iter()
            .chain(restored.markers_for(crate::protocol::AccountId(2)))
            .map(|marker| marker.id)
            .collect();
        restored.add(crate::protocol::AccountId(1), 7.0, 8.0);
        let new_id = restored
            .markers_for(crate::protocol::AccountId(1))
            .last()
            .unwrap()
            .id;
        assert!(!existing.contains(&new_id), "fresh id must be unique");
    }

    #[test]
    fn request_from_an_unknown_client_yields_nothing() {
        let mut server = crate::server::test_support::server();
        // No client connected with id 999, so there is no account to scope to.
        assert!(
            server
                .apply_world_map_request(crate::protocol::ClientId(999))
                .is_empty()
        );
    }
}
