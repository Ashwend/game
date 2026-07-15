//! Player-targeted admin commands: `/speed` (the run-speed cheat that scales the
//! issuing player's movement speed), `/health` (set your own HP, for testing the
//! damage/heal/vignette paths without needing someone to hit you), and
//! `/knockback-scale` (the global PvP knockback multiplier for combat-feel
//! tuning).

use crate::protocol::{ClientId, MAX_HEALTH};

use super::super::{GameServer, ServerEnvelope};
use super::{reply_success, reply_warning};

/// Clamp range for the `/speed` run-speed multiplier. The floor stays well
/// above zero so the command can't freeze the issuer; the ceiling keeps it a
/// fast-travel cheat rather than a physics-breaking teleport.
const MIN_RUN_SPEED_MULTIPLIER: f32 = 0.1;
const MAX_RUN_SPEED_MULTIPLIER: f32 = 20.0;

/// Floor for `/health`. Just above zero: dying has to go through the real death
/// path, not through a command that parks you at 0 HP while still `Alive`.
const MIN_SET_HEALTH: f32 = 1.0;

/// Clamp range for the `/knockback-scale` factor. `0.0` is allowed (knockback
/// off) so tuning can bracket the shipped feel from both sides; the ceiling
/// keeps a slammed slider from launching players across the map.
const MIN_KNOCKBACK_SCALE: f32 = 0.0;
const MAX_KNOCKBACK_SCALE: f32 = 5.0;

impl GameServer {
    /// `/speed <multiplier>`: scale the issuing player's movement speed.
    /// Movement is client-authoritative, so this just stores the multiplier
    /// (replicated to the owner via [`crate::server::PlayerInputAck`] and
    /// applied in their local prediction). Session-scoped: a fresh connection
    /// resets to `1.0`.
    pub(super) fn command_set_run_speed(
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
                "usage: /speed <multiplier> (e.g. /speed 2 for double run speed, /speed 1 to reset)",
            );
        };
        let Ok(multiplier) = token.parse::<f32>() else {
            return reply_warning(client_id, format!("could not parse '{token}' as a number"));
        };
        if !multiplier.is_finite() {
            return reply_warning(client_id, "multiplier must be a finite number");
        }

        let applied = multiplier.clamp(MIN_RUN_SPEED_MULTIPLIER, MAX_RUN_SPEED_MULTIPLIER);
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.run_speed_multiplier = applied;
        }
        reply_success(client_id, format!("run speed set to {applied:.2}x"))
    }

    /// `/health <value>`: set the issuing player's health, clamped to
    /// `(0, MAX_HEALTH]`. Admin-only.
    ///
    /// Exists because every other way to get to a specific HP requires someone (or
    /// something) to hit you for exactly the right amount, which makes the
    /// low-health vignette, the health bar, and the bandage heal awkward to check.
    /// The floor is deliberately just above zero rather than zero: killing yourself
    /// has to go through the real death path (`kill_player` drops your inventory
    /// and flips lifecycle), and a command that silently parked you at 0 HP while
    /// still `Alive` would be a desync, not a test.
    pub(super) fn command_set_health(
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
                "usage: /health <value> (e.g. /health 25 to test the low-health vignette)",
            );
        };
        let Ok(value) = token.parse::<f32>() else {
            return reply_warning(client_id, format!("could not parse '{token}' as a number"));
        };
        if !value.is_finite() {
            return reply_warning(client_id, "health must be a finite number");
        }

        let applied = value.clamp(MIN_SET_HEALTH, MAX_HEALTH);
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.controller.health = applied;
        }
        reply_success(client_id, format!("health set to {applied:.0}"))
    }

    /// `/knockback-scale <factor>`: scale global PvP knockback for combat-feel
    /// tuning (the Dev combat panel spells this command out next to its slider).
    /// Server-wide and non-persisted: the attack path multiplies the resolved
    /// knockback speed by this factor, and it resets to `1.0` on server restart.
    /// Admin-only, and it never touches persisted state.
    pub(super) fn command_set_knockback_scale(
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
                "usage: /knockback-scale <factor> (e.g. /knockback-scale 1.5, /knockback-scale 1 to reset)",
            );
        };
        let Ok(factor) = token.parse::<f32>() else {
            return reply_warning(client_id, format!("could not parse '{token}' as a number"));
        };
        if !factor.is_finite() {
            return reply_warning(client_id, "factor must be a finite number");
        }

        let applied = factor.clamp(MIN_KNOCKBACK_SCALE, MAX_KNOCKBACK_SCALE);
        self.knockback_scale = applied;
        reply_success(client_id, format!("knockback scale set to {applied:.2}x"))
    }
}
