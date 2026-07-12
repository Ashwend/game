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
mod player;
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
            "spawn" => self.command_spawn(client_id, &args),
            "drain" => self.command_drain(client_id, &args),
            "time" => self.command_set_time(client_id, &args),
            "speed" => self.command_set_run_speed(client_id, &args),
            "knockback-scale" | "knockbackscale" => {
                self.command_set_knockback_scale(client_id, &args)
            }
            "time-speed" | "timespeed" | "timescale" => {
                self.command_set_time_multiplier(client_id, &args)
            }
            "test-kit" | "testkit" => self.command_test_kit(client_id),
            "give" => self.command_give(client_id, &args),
            "tp" | "teleport" => self.command_teleport_all(client_id),
            "ruins" => self.command_ruins(client_id, &args),
            "meteor" => self.command_meteor_shower(client_id, &args),
            "meteor-here" | "meteorhere" => self.command_meteor_shower_here(client_id, &args),
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
        let spawn_line = if is_admin {
            "  /spawn <kind> [distance]: drop a resource node in front of you (e.g. /spawn pine 6)"
        } else {
            "  /spawn <kind> [distance]: admin only"
        };
        lines.push(spawn_line.to_owned());
        let drain_line = if is_admin {
            "  /drain [fraction]: set the looked-at node's remaining storage (e.g. /drain 0.4)"
        } else {
            "  /drain [fraction]: admin only"
        };
        lines.push(drain_line.to_owned());
        let time_line = if is_admin {
            "  /time <HH:MM|HHMM|hour>: set the time of day"
        } else {
            "  /time <HH:MM|HHMM|hour>: admin only"
        };
        lines.push(time_line.to_owned());
        let speed_line = if is_admin {
            "  /speed <multiplier>: set your run speed, a cheat (e.g. /speed 2, /speed 1 to reset)"
        } else {
            "  /speed <multiplier>: admin only"
        };
        lines.push(speed_line.to_owned());
        let knockback_line = if is_admin {
            "  /knockback-scale <factor>: scale PvP knockback for feel tuning (0 to 5, 1 to reset)"
        } else {
            "  /knockback-scale <factor>: admin only"
        };
        lines.push(knockback_line.to_owned());
        let time_speed_line = if is_admin {
            "  /time-speed <multiplier>: set the day/night cycle speed (0 to 240)"
        } else {
            "  /time-speed <multiplier>: admin only"
        };
        lines.push(time_speed_line.to_owned());
        let test_kit_line = if is_admin {
            "  /test-kit: grant every tool + 100 of each resource + 1 workbench + 1 furnace"
        } else {
            "  /test-kit: admin only"
        };
        lines.push(test_kit_line.to_owned());
        let give_line = if is_admin {
            "  /give <item_id|all> [count]: grant materials (default 1000, e.g. /give stone, /give all)"
        } else {
            "  /give <item_id|all> [count]: admin only"
        };
        lines.push(give_line.to_owned());
        let tp_line = if is_admin {
            "  /tp: teleport every other connected player to your position (for PvP/death testing)"
        } else {
            "  /tp: admin only"
        };
        lines.push(tp_line.to_owned());
        let ruins_line = if is_admin {
            "  /ruins [tp]: list the nearest ruins with distances; /ruins tp warps you to the nearest"
        } else {
            "  /ruins [tp]: admin only"
        };
        lines.push(ruins_line.to_owned());
        let meteor_shower_line = if is_admin {
            "  /meteor [warning_seconds]: force a meteor now (default 30s to impact)"
        } else {
            "  /meteor [warning_seconds]: admin only"
        };
        lines.push(meteor_shower_line.to_owned());
        let meteor_shower_here_line = if is_admin {
            "  /meteor-here [warning_seconds]: drop a meteor on your position (default 8s, can hit you/bases)"
        } else {
            "  /meteor-here [warning_seconds]: admin only"
        };
        lines.push(meteor_shower_here_line.to_owned());

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
