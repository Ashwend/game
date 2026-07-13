//! Server authority for defusing a placed explosive charge, the defender's
//! counterplay to a raid.
//!
//! A live charge (`DeployableKind::Explosive`) hisses and glows through its
//! fuse; a defender who reaches it can hold-E to defuse it. Defusing removes the
//! charge WITHOUT detonating it and refunds half its recipe materials (rounded
//! down, per input) to the defuser, overflow dropping at their feet. Shooting a
//! charge to 0 HP fizzles it (no refund, in the deployable damage path); this is
//! the deliberate, cheaper counter, hence the half refund and the claim gate.
//!
//! ## Who may defuse (the claim rule)
//!
//! A charge inside a Tool Cupboard claim can be defused only by a player
//! authorized on that claim; a charge outside any claim can be defused by anyone
//! in reach. This is exactly [`GameServer::claim_blocks_placement`] measured at
//! the charge's position: it is `false` (defuse allowed) when the ground is
//! unclaimed OR the requester is authorized, and `true` (defuse rejected) only
//! when a claim covers the charge and the requester is not on its list.
//!
//! The rationale is anti-grief symmetric with placement: a raider's charge
//! landed inside a defender's claimed base is the defenders' to disarm, while a
//! charge set out in the open (no claim) is fair game for anyone nearby to pull.
//! The placer is not special-cased: once a charge is armed, only claim
//! authorization (or an open field) decides who can pull it, so a raider cannot
//! rely on being the one to safely retrieve a mis-thrown charge inside a base
//! they have no claim on.

use crate::{
    crafting::recipe_for_output,
    game_balance::{
        EXPLOSIVE_DEFUSE_REACH_M, EXPLOSIVE_DEFUSE_REFUND_DENOMINATOR,
        EXPLOSIVE_DEFUSE_REFUND_NUMERATOR,
    },
    items::DeployableKind,
    protocol::{ClientId, DeployedEntityId, ItemStack},
};

use super::{
    GameServer, ServerEnvelope,
    inventory::add_stack_to_inventory,
    movement::drop_origin_for,
    toasts::{success, warn},
};

/// Compute the half-refund (rounded down, per input) for the charge that crafts
/// into `item_id`. Pure so the refund math is unit-testable without a server:
/// each recipe input contributes `floor(quantity * NUM / DEN)` of its item,
/// dropping inputs that round to zero. Returns an empty vec when nothing crafts
/// into `item_id` (a world-spawned kind, which no charge is).
pub(super) fn defuse_refund_for(item_id: &str) -> Vec<ItemStack> {
    let Some(recipe) = recipe_for_output(item_id) else {
        return Vec::new();
    };
    recipe
        .inputs
        .iter()
        .filter_map(|input| {
            // u32 math so a large quantity * numerator can't overflow u16 before
            // the divide; the result is always <= the original quantity.
            let refund = (input.quantity as u32 * EXPLOSIVE_DEFUSE_REFUND_NUMERATOR as u32)
                / EXPLOSIVE_DEFUSE_REFUND_DENOMINATOR as u32;
            (refund > 0).then(|| ItemStack::new(input.item_id, refund as u16))
        })
        .collect()
}

impl GameServer {
    /// Defuse the placed charge `id` for `client_id`. Validates existence, that
    /// it is a live explosive charge, that the requester is in reach, and the
    /// claim rule (see the module docs). On success removes the charge without
    /// detonation and refunds half its recipe materials, dropping any overflow.
    /// Always answers with a toast (success or the specific rejection reason).
    pub(super) fn defuse_charge(
        &mut self,
        client_id: ClientId,
        id: DeployedEntityId,
    ) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if client.lifecycle.is_dead() {
            return Vec::new();
        }
        let account = client.account_id;
        let requester_position = client.controller.position;

        // Existence + kind: must be a live explosive charge.
        let Some(entity) = self.deployed_entities.get(&id) else {
            return warn(client_id, "That charge is already gone");
        };
        if !matches!(entity.kind, DeployableKind::Explosive { .. }) {
            return Vec::new();
        }
        let charge_item = match entity.kind {
            DeployableKind::Explosive { kind } => kind.item_id(),
            _ => unreachable!("kind checked to be Explosive above"),
        };
        let charge_position = entity.position;
        let charge_blocks = entity.resolved_collider_blocks();

        // Reach: measured to the charge collider surface, so a wall-stuck ember
        // charge is reachable from the wall side, matching how the client aims.
        if !super::deployables::within_horizontal_range_of_blocks(
            requester_position,
            &charge_blocks,
            EXPLOSIVE_DEFUSE_REACH_M,
        ) {
            return warn(client_id, "Get closer to defuse the charge");
        }

        // Claim rule: a charge inside a claim can be defused only by an
        // authorized player; a charge outside any claim by anyone. See module
        // docs. `claim_blocks_placement` is true only when a claim covers the
        // charge and this account is not on its authorized list.
        if self.claim_blocks_placement(charge_position, account) {
            return warn(
                client_id,
                "Only authorized players can defuse a charge in this claim",
            );
        }

        // Passed every gate: remove the charge WITHOUT detonating it (no blast,
        // no fizzle toast, no content spill; a charge stores nothing), then
        // refund half the recipe materials into the defuser's inventory.
        self.remove_deployed_entity_tracked(id);

        let refund = defuse_refund_for(charge_item);
        let mut overflow = Vec::new();
        if let Some(client) = self.clients.get_mut(&client_id) {
            for stack in refund {
                if let Some(remainder) = add_stack_to_inventory(&mut client.inventory, stack) {
                    overflow.push(remainder);
                }
            }
        }
        // Overflow drops at the defuser's feet, the same recovery path craft
        // refunds and container ejects use, so a full bag never eats materials.
        if !overflow.is_empty() {
            let origin = {
                let client = self.clients.get(&client_id).expect("client resolved above");
                drop_origin_for(client)
            };
            for stack in overflow {
                self.spawn_dropped_item_at(origin, stack);
            }
        }

        success(client_id, "Charge defused, materials recovered")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{POWDER_KEG_ID, SATCHEL_CHARGE_ID};

    #[test]
    fn refund_is_exactly_half_each_input_rounded_down() {
        // Powder keg recipe: 30 gunpowder + 15 wood + 2 plant_twine.
        // Half (floor): 15 gunpowder + 7 wood + 1 plant_twine.
        let refund = defuse_refund_for(POWDER_KEG_ID);
        let get = |id: &str| {
            refund
                .iter()
                .find(|s| s.item_id.as_ref() == id)
                .map(|s| s.quantity)
        };
        assert_eq!(get(crate::items::GUNPOWDER_ID), Some(15));
        assert_eq!(get(crate::items::WOOD_ID), Some(7));
        assert_eq!(get(crate::items::PLANT_TWINE_ID), Some(1));
    }

    #[test]
    fn refund_drops_inputs_that_round_to_zero() {
        // A single-unit input halved rounds to zero and is dropped from the
        // refund entirely (never a zero-quantity stack). The satchel's inputs
        // are all >= 2, so its refund keeps every line; assert that nothing
        // yields a zero stack.
        for stack in defuse_refund_for(SATCHEL_CHARGE_ID) {
            assert!(stack.quantity > 0, "refund must never carry a zero stack");
        }
    }

    #[test]
    fn satchel_charge_refund_is_half_of_every_input() {
        // Satchel recipe: 60 gunpowder + 4 cloth + 2 fittings.
        // Half (floor): 30 gunpowder + 2 cloth + 1 fittings.
        let refund = defuse_refund_for(SATCHEL_CHARGE_ID);
        let get = |id: &str| {
            refund
                .iter()
                .find(|s| s.item_id.as_ref() == id)
                .map(|s| s.quantity)
        };
        assert_eq!(get(crate::items::GUNPOWDER_ID), Some(30));
        assert_eq!(get(crate::items::CLOTH_ID), Some(2));
        assert_eq!(get(crate::items::SALVAGED_FITTINGS_ID), Some(1));
    }

    #[test]
    fn unknown_output_refunds_nothing() {
        assert!(defuse_refund_for("not_a_real_item").is_empty());
    }
}
