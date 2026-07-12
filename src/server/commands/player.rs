//! Player-targeted admin commands: `/speed` (the run-speed cheat that scales the
//! issuing player's movement speed) and `/knockback-scale` (the global PvP
//! knockback multiplier for combat-feel tuning).

use crate::protocol::ClientId;

use super::super::{GameServer, ServerEnvelope};
use super::{reply_success, reply_warning};

/// Clamp range for the `/speed` run-speed multiplier. The floor stays well
/// above zero so the command can't freeze the issuer; the ceiling keeps it a
/// fast-travel cheat rather than a physics-breaking teleport.
const MIN_RUN_SPEED_MULTIPLIER: f32 = 0.1;
const MAX_RUN_SPEED_MULTIPLIER: f32 = 20.0;

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
