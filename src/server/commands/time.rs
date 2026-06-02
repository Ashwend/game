//! `/time` and `/speed`, world-time admin commands.

use crate::{
    protocol::{ClientId, ServerMessage, ToastKind, ToastMessage},
    world_time::{MAX_MULTIPLIER, MIN_MULTIPLIER, parse_time_token},
};

use super::super::{DeliveryTarget, GameServer, ServerEnvelope};
use super::reply_warning;

impl GameServer {
    pub(super) fn command_set_time(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        let Some(token) = args.first() else {
            return reply_warning(
                client_id,
                "usage: /time <HH:MM>, /time <HHMM>, or /time <hour>",
            );
        };
        let Some(seconds) = parse_time_token(token) else {
            return reply_warning(
                client_id,
                format!(
                    "could not parse '{token}'; try '/time 06:30', '/time 0700', or '/time 14'"
                ),
            );
        };

        self.set_world_time_seconds(seconds);
        let label = self.world_time.format_hhmm();
        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            },
            ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Toast(ToastMessage::new(
                    ToastKind::Success,
                    format!("time set to {label}"),
                )),
            },
        ]
    }

    pub(super) fn command_set_time_multiplier(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        let Some(token) = args.first() else {
            return reply_warning(
                client_id,
                format!("usage: /speed <multiplier> (0 to {MAX_MULTIPLIER})"),
            );
        };
        let Ok(multiplier) = token.parse::<f32>() else {
            return reply_warning(client_id, format!("could not parse '{token}' as a number"));
        };
        if !multiplier.is_finite() || multiplier < MIN_MULTIPLIER {
            return reply_warning(
                client_id,
                format!("multiplier must be in [{MIN_MULTIPLIER}, {MAX_MULTIPLIER}]"),
            );
        }

        self.set_world_time_multiplier(multiplier);
        let applied = self.world_time.multiplier;
        vec![
            ServerEnvelope {
                target: DeliveryTarget::Broadcast,
                message: ServerMessage::WorldTime(self.world_time_snapshot()),
            },
            ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Toast(ToastMessage::new(
                    ToastKind::Success,
                    format!("day/night speed set to {applied:.2}×"),
                )),
            },
        ]
    }
}
