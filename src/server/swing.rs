//! Cosmetic swing-start handling.
//!
//! A swing is otherwise invisible to the server until the impact frame (the
//! gather/attack/damage commands fire then, and never on whiffs). To let peers
//! play a matching third-person swing on the rigged body, the client sends a
//! `SwingStart` the instant a swing begins; this stamps the swinger's
//! peer-visible [`crate::server::PlayerAction`] (replicated, not a
//! `ServerMessage`). Purely visual: it never touches gameplay state.

use crate::{
    items::item_definition,
    protocol::{ClientId, SwingStartCommand},
};

use super::{GameServer, ServerEnvelope};

impl GameServer {
    /// Record a freshly-started swing for the cosmetic peer animation.
    ///
    /// `swing_seq` advances monotonically (one bump per genuinely new swing),
    /// so peers can edge-detect it; `max` guards against a stale or duplicate
    /// `SwingStart` rewinding it. The swing archetype (`model`) is re-derived
    /// from the authoritative actionbar so the animation always matches what the
    /// player is actually holding: a dedicated weapon its own registry model, a
    /// gather tool its archetype. The wire `command.model` is only a fallback for
    /// the rare case the held item carries neither profile. Rejected for a dead
    /// body. Returns no envelopes: the state ships through the replicated
    /// component.
    pub(super) fn apply_swing_start(
        &mut self,
        client_id: ClientId,
        command: SwingStartCommand,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if !client.lifecycle.is_alive() {
            return Vec::new();
        }

        let model = client
            .inventory
            .active_actionbar_stack()
            .and_then(|stack| item_definition(&stack.item_id))
            .map(|definition| definition.swing_model())
            .unwrap_or(command.model);

        let next_seq = client.swing_seq.max(command.seq);
        // Only mutate on a genuinely newer swing (or a model change) so the
        // mirror's compare-and-write doesn't re-ship an identical PlayerAction.
        if next_seq != client.swing_seq {
            client.swing_seq = next_seq;
            client.swing_model = model;
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        items::ItemModel,
        protocol::SwingStartCommand,
        server::{
            PlayerLifecycle,
            test_support::{connect_host, equip_basic_tools, server},
        },
    };

    #[test]
    fn swing_start_bumps_seq_and_records_model() {
        let mut server = server();
        let host = connect_host(&mut server);
        equip_basic_tools(&mut server, host); // hatchet in slot 0
        server
            .clients
            .get_mut(&host)
            .unwrap()
            .inventory
            .active_actionbar_slot = 0;

        server.apply_swing_start(
            host,
            SwingStartCommand {
                seq: 1,
                model: ItemModel::Hatchet,
            },
        );

        let client = server.clients.get(&host).expect("client");
        assert_eq!(client.swing_seq, 1);
        // Model comes from the authoritative actionbar (the hatchet => Hatchet),
        // not blindly from the wire.
        assert_eq!(client.swing_model, ItemModel::Hatchet);
    }

    #[test]
    fn swing_start_uses_authoritative_model_over_a_spoofed_one() {
        let mut server = server();
        let host = connect_host(&mut server);
        equip_basic_tools(&mut server, host); // actionbar slot 0 = hatchet (Hatchet)
        server
            .clients
            .get_mut(&host)
            .unwrap()
            .inventory
            .active_actionbar_slot = 0;

        // Client lies that it is swinging a mace; the server overrides from the
        // actionbar, which holds the hatchet.
        server.apply_swing_start(
            host,
            SwingStartCommand {
                seq: 1,
                model: ItemModel::Mace,
            },
        );

        assert_eq!(
            server.clients.get(&host).unwrap().swing_model,
            ItemModel::Hatchet
        );
    }

    #[test]
    fn swing_start_seq_never_rewinds() {
        let mut server = server();
        let host = connect_host(&mut server);
        equip_basic_tools(&mut server, host);

        server.apply_swing_start(
            host,
            SwingStartCommand {
                seq: 5,
                model: ItemModel::Hatchet,
            },
        );
        // A stale, lower seq must not rewind the counter (peers would replay).
        server.apply_swing_start(
            host,
            SwingStartCommand {
                seq: 2,
                model: ItemModel::Hatchet,
            },
        );

        assert_eq!(server.clients.get(&host).unwrap().swing_seq, 5);
    }

    #[test]
    fn players_iter_reports_equipped_held_mesh() {
        use crate::items::HeldMesh;
        let mut server = server();
        let host = connect_host(&mut server);
        equip_basic_tools(&mut server, host); // hatchet in slot 0
        server
            .clients
            .get_mut(&host)
            .unwrap()
            .inventory
            .active_actionbar_slot = 0;

        let view = server
            .players_iter()
            .find(|view| view.client_id == host)
            .expect("host view");
        assert_eq!(view.held.0, Some(HeldMesh::StoneHatchet));

        // An empty active slot reads as an empty hand.
        server
            .clients
            .get_mut(&host)
            .unwrap()
            .inventory
            .active_actionbar_slot = 5;
        let view = server
            .players_iter()
            .find(|view| view.client_id == host)
            .expect("host view");
        assert_eq!(view.held.0, None);
    }

    #[test]
    fn dead_player_swing_start_is_ignored() {
        let mut server = server();
        let host = connect_host(&mut server);
        equip_basic_tools(&mut server, host);
        server.clients.get_mut(&host).unwrap().lifecycle = PlayerLifecycle::Dead {
            since_tick: 0,
            killer: None,
        };

        server.apply_swing_start(
            host,
            SwingStartCommand {
                seq: 1,
                model: ItemModel::Hatchet,
            },
        );

        assert_eq!(server.clients.get(&host).unwrap().swing_seq, 0);
    }
}
