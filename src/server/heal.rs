//! Server-authoritative healing: the consumable use charge (bandage) and the
//! heal-over-time it leaves behind.
//!
//! ## Why the server owns the charge
//!
//! The client never tells us a use *finished*. It sends `UseStart` and
//! `UseCancel`; we stamp the tick the start arrived and re-derive the fraction
//! from our own clock every tick, applying the effect ourselves the moment it
//! completes. The bow can afford to take a client `Fire` because a forged early
//! release only ever deals *less* damage; "I finished my bandage" has no such
//! gradient, so accepting it would be handing out free instant heals. See
//! [`crate::protocol::ConsumableCommand`].
//!
//! ## One heal tail, mirroring the one damage tail
//!
//! Every damage source in the game funnels through `apply_player_damage`
//! (`super::combat`). Healing mirrors that exactly: the instant chunk and the
//! per-tick trickle both go through [`GameServer::apply_player_heal`], so the
//! clamp to `MAX_HEALTH` and the don't-heal-a-corpse rule are stated once and
//! cannot be forgotten by a future caller.
//!
//! ## The replication-traffic trap
//!
//! `PlayerHealth` replicates on a `PartialEq` compare-and-write
//! (`net::host::mirror`). A naive heal-over-time adds a fraction of a point every
//! tick, so the value changes *every tick* and ships a diff at 20 Hz per healing
//! player for the whole window. Instead the fractional remainder accumulates in a
//! server-only field ([`HealOverTime::pending`]) and only lands on
//! `controller.health` when it has built up to a whole point. A 20 HP / 10 s
//! bandage therefore ships ~20 diffs instead of ~200, and the player still sees a
//! smooth-enough climb.

use crate::items::ConsumableProfile;
use crate::protocol::{ClientId, ConsumableCommand, MAX_HEALTH};

use super::{GameServer, ServerClient, ServerEnvelope};

/// A heal-over-time in flight on one player. Server-only: it never replicates
/// (only its effect on `controller.health` does), and it deliberately does not
/// persist across a save (see the module docs on `PersistedPlayer`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HealOverTime {
    /// Health still owed to the player by this heal.
    pub(super) remaining: f32,
    /// Health paid out per tick.
    pub(super) per_tick: f32,
    /// Sub-point remainder not yet written to `controller.health`. This is the
    /// whole reason the HoT doesn't spam a replication diff every tick.
    pub(super) pending: f32,
}

impl GameServer {
    /// Handle a `ClientMessage::Consumable`.
    ///
    /// Neither variant returns envelopes: a use that starts is pure server state
    /// (plus a movement slow, which replicates through the existing
    /// `run_speed_multiplier` lever), and a use that *completes* reaches the
    /// client through the replicated `PlayerHealth` diff. There is deliberately
    /// no bespoke "you were healed" message.
    pub(super) fn apply_consumable_command(
        &mut self,
        client_id: ClientId,
        command: ConsumableCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            ConsumableCommand::UseStart => self.begin_consumable_use(client_id),
            ConsumableCommand::UseCancel => {
                self.clear_consumable_use(client_id);
                Vec::new()
            }
        }
    }

    /// Begin a use: validate the held item is actually a consumable and the
    /// player is alive, then record the start tick and slow movement. A no-op
    /// otherwise, so a client can't slow-walk (or, worse, arm a heal) with a
    /// rock in hand.
    ///
    /// Note it does NOT consume the item here. The bandage is only spent when the
    /// charge completes, which is what makes an early release genuinely free.
    fn begin_consumable_use(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(profile) = self.held_consumable(client_id) else {
            return Vec::new();
        };
        let tick = self.tick;
        if let Some(client) = self.clients.get_mut(&client_id) {
            // A corpse can't bandage itself.
            if client.lifecycle.is_dead() {
                return Vec::new();
            }
            client.use_started_tick = Some(tick);
            client.run_speed_multiplier = profile.use_move_multiplier;
        }
        Vec::new()
    }

    /// Clear any active use and restore movement. Idempotent. This is the single
    /// restore path every exit funnels through (completion, cancel, item swap,
    /// death), so the move multiplier can never get stuck at the use value.
    ///
    /// Mirrors `clear_ranged_draw`, including the `.take().is_some()` guard: only
    /// restore the multiplier if we actually cleared a use, so this never stomps a
    /// concurrently-set admin `/speed` when no use was running.
    pub(super) fn clear_consumable_use(&mut self, client_id: ClientId) {
        if let Some(client) = self.clients.get_mut(&client_id)
            && client.use_started_tick.take().is_some()
        {
            client.run_speed_multiplier = 1.0;
        }
    }

    /// The `ConsumableProfile` of the client's active actionbar item, if it has
    /// one. `None` for every other item and for an empty hand.
    fn held_consumable(&self, client_id: ClientId) -> Option<ConsumableProfile> {
        let client = self.clients.get(&client_id)?;
        let stack = client.inventory.active_actionbar_stack()?;
        crate::items::item_definition(&stack.item_id)?.consumable
    }

    /// Advance every in-flight consumable use one tick, applying the ones whose
    /// charge completed on OUR clock.
    ///
    /// Envelope-free by design: the heal lands on `controller.health` and reaches
    /// both the local HUD and peer nameplates through the replicated
    /// `PlayerHealth` diff.
    pub(super) fn tick_consumable_uses(&mut self) {
        let tick = self.tick;

        // Collect first: applying a use mutates the client (inventory + health),
        // so we can't hold an iterator over `self.clients` across it.
        let completed: Vec<(ClientId, ConsumableProfile)> = self
            .clients
            .iter()
            .filter_map(|(&client_id, client)| {
                let started = client.use_started_tick?;
                if client.lifecycle.is_dead() || !client.online {
                    return None;
                }
                let stack = client.inventory.active_actionbar_stack()?;
                let profile = crate::items::item_definition(&stack.item_id)?.consumable?;
                profile
                    .use_completes(tick.saturating_sub(started))
                    .then_some((client_id, profile))
            })
            .collect();

        for (client_id, profile) in completed {
            self.complete_consumable_use(client_id, profile);
        }
    }

    /// Spend the item and land its effect. Called only from
    /// [`Self::tick_consumable_uses`], only once the charge has genuinely
    /// completed on the server's own clock.
    fn complete_consumable_use(&mut self, client_id: ClientId, profile: ConsumableProfile) {
        // Clear the charge FIRST, before any fallible step, so no early return
        // below can leave the player stuck at the use movement multiplier with a
        // charge that will re-complete on the very next tick. (Same clear-first
        // ordering, and the same reason, as `fire_ranged`.)
        self.clear_consumable_use(client_id);

        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        let Some(stack) = client.inventory.active_actionbar_stack() else {
            return;
        };
        let item_id = stack.item_id.to_string();

        // Spend exactly one. If the stack vanished mid-charge (dropped into a
        // box, traded away), the heal does not happen: no item, no effect.
        let removed =
            crate::inventory::take_items_from_inventory(&mut client.inventory, &item_id, 1);
        if removed != 1 {
            return;
        }

        // Arm the over-time remainder. A second bandage REFRESHES rather than
        // stacks: the remainder is replaced, not added to, so chain-using them
        // can never bank a bigger trickle than one bandage's worth. Stacking would
        // make holding a stack strictly better than spacing them out, which is the
        // opposite of the tempo decision the item is for.
        client.heal_over_time = (profile.heal_over_time > 0.0 && profile.heal_duration_ticks > 0)
            .then(|| HealOverTime {
                remaining: profile.heal_over_time,
                per_tick: profile.heal_per_tick(),
                pending: 0.0,
            });

        self.apply_player_heal(client_id, profile.instant_heal);
    }

    /// Pay out every in-flight heal-over-time one tick's worth.
    ///
    /// The fractional trickle accumulates in `pending` and is only committed to
    /// `controller.health` once it reaches a whole point, so a 10-second heal
    /// ships ~20 replication diffs instead of ~200. The final tick flushes
    /// whatever is left, so the player is never shortchanged by the rounding.
    pub(super) fn tick_heal_over_time(&mut self) {
        let healing: Vec<(ClientId, f32)> = self
            .clients
            .iter_mut()
            .filter_map(|(&client_id, client)| {
                // A corpse (or a logged-out sleeping body) does not regenerate.
                // Dropping the HoT outright, rather than pausing it, means dying
                // mid-trickle forfeits the rest, which is the intended cost.
                if client.lifecycle.is_dead() || !client.online {
                    client.heal_over_time = None;
                    return None;
                }
                let hot = client.heal_over_time.as_mut()?;

                let step = hot.per_tick.min(hot.remaining);
                hot.remaining -= step;
                hot.pending += step;
                let done = hot.remaining <= f32::EPSILON;

                // Commit on a whole point, or on the last tick (flush the dust).
                let commit = if done || hot.pending >= 1.0 {
                    std::mem::take(&mut hot.pending)
                } else {
                    0.0
                };
                if done {
                    client.heal_over_time = None;
                }
                (commit > 0.0).then_some((client_id, commit))
            })
            .collect();

        for (client_id, amount) in healing {
            self.apply_player_heal(client_id, amount);
        }
    }

    /// The single heal tail. Every health *increase* outside a respawn goes
    /// through here, mirroring `apply_player_damage` on the damage side.
    ///
    /// Clamps to `MAX_HEALTH` itself: the clamp in `PlayerController::simulate_step`
    /// is client-only (the server never simulates movement, it just holds the
    /// controller as a state bag), so without this we would happily replicate a
    /// 130 HP player to the HUD and every nameplate.
    ///
    /// Refuses to touch a corpse. `kill_player` locks a dead player at exactly
    /// `0.0` alongside `PlayerLifecycle::Dead`; healing one back above zero would
    /// resurrect them without going through the respawn path and desync lifecycle
    /// from health.
    pub(super) fn apply_player_heal(&mut self, client_id: ClientId, amount: f32) {
        if !amount.is_finite() || amount <= 0.0 {
            return;
        }
        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        if client.lifecycle.is_dead() {
            return;
        }
        client.controller.health = (client.controller.health + amount).min(MAX_HEALTH);
    }
}

impl ServerClient {
    /// The client's live use-charge fraction in `[0, 1]`, or `0.0` when no use is
    /// running. Feeds the replicated peer-visible charge so other players can see
    /// someone mid-bandage (and know they are a soft target).
    pub(super) fn consumable_use_fraction(&self, tick: u64) -> f32 {
        let Some(started) = self.use_started_tick else {
            return 0.0;
        };
        let Some(stack) = self.inventory.active_actionbar_stack() else {
            return 0.0;
        };
        crate::items::item_definition(&stack.item_id)
            .and_then(|definition| definition.consumable)
            .map(|profile| profile.use_fraction(tick.saturating_sub(started)))
            .unwrap_or(0.0)
    }
}
