//! Server-side `/command` handling.
//!
//! Slash commands are typed in chat and shipped to the server as
//! `ClientMessage::Command { text }`. The server is the source of truth for
//! parsing, the admin check, and any state mutation. The client only knows
//! how to tell chat input apart from command input by the leading `/`.
//!
//! Each command yields a `Vec<ServerEnvelope>` like the rest of the receive
//! path, a Toast (back to the issuer) plus any side-effects (resource node
//! insert, broadcast snapshot pickup on the next tick, etc.).

mod kit;
mod time;
mod world;

#[cfg(test)]
mod tests;

use crate::protocol::{
    ChatMessage, ClientId, ResourceNodeId, ServerMessage, ToastKind, ToastMessage,
};

use super::{DeliveryTarget, GameServer, ServerEnvelope};

impl GameServer {
    /// Apply a `ClientMessage::Command` payload. Trims the leading slash if
    /// the client forgot to strip it, splits on whitespace, and dispatches
    /// to per-command handlers.
    pub(super) fn apply_command(
        &mut self,
        client_id: ClientId,
        text: String,
    ) -> Vec<ServerEnvelope> {
        let trimmed = text.trim().trim_start_matches('/');
        if trimmed.is_empty() {
            return reply_warning(client_id, "empty command");
        }

        let mut parts = trimmed.split_whitespace();
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        let args: Vec<&str> = parts.collect();

        match name.as_str() {
            "spawn-ore" | "spawnore" => self.command_spawn_ore(client_id, &args),
            "time" => self.command_set_time(client_id, &args),
            "speed" | "timescale" => self.command_set_time_multiplier(client_id, &args),
            "test-kit" | "testkit" => self.command_test_kit(client_id),
            "tp" | "teleport" => self.command_teleport_all(client_id),
            "help" => self.command_help(client_id),
            other => reply_warning(client_id, format!("unknown command: /{other}")),
        }
    }

    /// `/help`, drop the command list into the issuer's chat log as
    /// messages from "Server" (rather than a toast) so it lingers, scrolls,
    /// and reads alongside normal conversation. Only the issuer sees it.
    fn command_help(&self, client_id: ClientId) -> Vec<ServerEnvelope> {
        // Whether each line is admin-only. Non-admins still see the section
        // but the rendered list tells them what's gated, instead of leaving
        // the impression that nothing exists.
        let is_admin = self
            .clients
            .get(&client_id)
            .map(|client| client.is_admin)
            .unwrap_or(false);

        let mut lines: Vec<String> = vec!["Available commands:".to_owned()];
        lines.push("  /help: show this list".to_owned());
        let spawn_ore_line = if is_admin {
            "  /spawn-ore [coal|iron|sulfur] [radius]: drop a fresh ore node nearby"
        } else {
            "  /spawn-ore [coal|iron|sulfur] [radius]: admin only"
        };
        lines.push(spawn_ore_line.to_owned());
        let time_line = if is_admin {
            "  /time <HH:MM|HHMM|hour>: set the time of day"
        } else {
            "  /time <HH:MM|HHMM|hour>: admin only"
        };
        lines.push(time_line.to_owned());
        let speed_line = if is_admin {
            "  /speed <multiplier>: set the day/night speed (0 to 240)"
        } else {
            "  /speed <multiplier>: admin only"
        };
        lines.push(speed_line.to_owned());
        let test_kit_line = if is_admin {
            "  /test-kit: grant every tool + 100 of each resource + 1 workbench + 1 furnace"
        } else {
            "  /test-kit: admin only"
        };
        lines.push(test_kit_line.to_owned());
        let tp_line = if is_admin {
            "  /tp: teleport every other connected player to your position (for PvP/death testing)"
        } else {
            "  /tp: admin only"
        };
        lines.push(tp_line.to_owned());

        lines
            .into_iter()
            .map(|text| ServerEnvelope {
                target: DeliveryTarget::Client(client_id),
                message: ServerMessage::Chat(ChatMessage {
                    from: "Server".to_owned(),
                    text,
                }),
            })
            .collect()
    }

    pub(super) fn allocate_resource_node_id(&mut self) -> ResourceNodeId {
        let id = self.next_resource_node_id;
        self.next_resource_node_id = self.next_resource_node_id.saturating_add(1);
        id
    }
}

pub(super) fn reply_success(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Success, text)),
    }]
}

pub(super) fn reply_warning(client_id: ClientId, text: impl Into<String>) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(ToastKind::Warning, text)),
    }]
}
