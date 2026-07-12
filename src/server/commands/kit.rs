//! `/test-kit` and `/give`, debug commands that grant items.

use crate::{
    items::{
        ANCIENT_FITTINGS_ID, ARROW_ID, BASIC_HATCHET_ID, BASIC_PICKAXE_ID, BUILDING_PLAN_ID,
        CLOTH_ID, COAL_ID, CROSSBOW_ID, CRUDE_FURNACE_ID, FIBER_ID, GUNPOWDER_ID, HAMMER_ID,
        HEWN_LOG_DOOR_ID, HEWN_LOG_ID, IRON_BAR_ID, IRON_BOOTS_ID, IRON_CUIRASS_ID,
        IRON_GREAVES_ID, IRON_HATCHET_ID, IRON_HELM_ID, IRON_MACE_ID, IRON_ORE_ID, IRON_PICKAXE_ID,
        IRON_SWORD_ID, LAMELLAR_BOOTS_ID, LAMELLAR_GREAVES_ID, LAMELLAR_HELM_ID, LAMELLAR_VEST_ID,
        METEORITE_ID, PADDED_HOOD_ID, PADDED_LEGGINGS_ID, PADDED_TUNIC_ID, PADDED_WRAPS_ID,
        PLANT_TWINE_ID, POWDER_BOMB_ID, POWDER_KEG_ID, SATCHEL_CHARGE_ID, SLEEPING_BAG_ID,
        STONE_ID, STONE_SPEAR_ID, SULFUR_ID, SULFUR_ORE_ID, WOOD_ID, WOODEN_BOW_ID, WOODEN_CLUB_ID,
        WORKBENCH_T1_ID, item_definition, stack_limit,
    },
    protocol::{ClientId, ItemStack},
};

use super::super::{GameServer, ServerEnvelope, inventory::add_stack_to_inventory};
use super::{reply_success, reply_warning};

impl GameServer {
    /// `/test-kit`, debug shortcut that fills the player's bag with the
    /// full early-game kit:
    ///
    /// - Equipables (tools + deployables) → first empty actionbar slot,
    ///   falling back to inventory if the actionbar is already packed.
    /// - Resources (100 of each material) → first empty inventory slot
    ///   so they don't shove existing actionbar contents around.
    ///
    /// Admin only. Any items that can't fit (e.g. inventory full from
    /// earlier kits) are reported in the success toast, no silent loss.
    pub(super) fn command_test_kit(&mut self, client_id: ClientId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }

        // (item_id, quantity) tuples. Tools, deployables, and worn armor are
        // all equipables and go to the actionbar first (falling back to the
        // inventory grid once the nine actionbar slots fill); resources go
        // straight to the inventory grid. All twelve armor pieces (padded,
        // lamellar, iron) land as stacks of one alongside the tools so a tester
        // can drag a full set onto the paperdoll and see it on the rig. The kit
        // has 31 equipables + 16 resources = 47 items, well within the 9
        // actionbar + 60 inventory slots, so nothing overflows.
        const EQUIPABLES: &[&str] = &[
            BASIC_HATCHET_ID,
            BASIC_PICKAXE_ID,
            IRON_HATCHET_ID,
            IRON_PICKAXE_ID,
            // The four melee weapons, so a tester can pull them onto
            // the actionbar and swing without crafting.
            WOODEN_CLUB_ID,
            STONE_SPEAR_ID,
            IRON_SWORD_ID,
            IRON_MACE_ID,
            // The two ranged weapons; arrows ride the RESOURCES list below.
            WOODEN_BOW_ID,
            CROSSBOW_ID,
            WORKBENCH_T1_ID,
            CRUDE_FURNACE_ID,
            BUILDING_PLAN_ID,
            HAMMER_ID,
            HEWN_LOG_DOOR_ID,
            SLEEPING_BAG_ID,
            // The three armor sets: padded (hand), lamellar (workbench 1), iron
            // (workbench 2). One piece per slot per set.
            PADDED_HOOD_ID,
            PADDED_TUNIC_ID,
            PADDED_LEGGINGS_ID,
            PADDED_WRAPS_ID,
            LAMELLAR_HELM_ID,
            LAMELLAR_VEST_ID,
            LAMELLAR_GREAVES_ID,
            LAMELLAR_BOOTS_ID,
            IRON_HELM_ID,
            IRON_CUIRASS_ID,
            IRON_GREAVES_ID,
            IRON_BOOTS_ID,
            // The three explosives, so a tester can throw the bomb and place
            // the keg / satchel to raid without crafting.
            POWDER_BOMB_ID,
            POWDER_KEG_ID,
            SATCHEL_CHARGE_ID,
        ];
        // Wood appears twice on purpose: a starter base (foundation +
        // four wall pieces) costs more than one 100-stack. The
        // intermediates (refined sulfur, gunpowder, cloth) and the two
        // rare, uncraftable exploration resources (meteorite, ancient
        // fittings) ride along so a tester can craft and upgrade without
        // farming them.
        const RESOURCES: &[&str] = &[
            WOOD_ID,
            WOOD_ID,
            STONE_ID,
            COAL_ID,
            IRON_ORE_ID,
            SULFUR_ORE_ID,
            SULFUR_ID,
            GUNPOWDER_ID,
            FIBER_ID,
            CLOTH_ID,
            PLANT_TWINE_ID,
            IRON_BAR_ID,
            HEWN_LOG_ID,
            METEORITE_ID,
            ANCIENT_FITTINGS_ID,
            // Arrows for the bow and crossbow, a full stack (clamped to 24).
            ARROW_ID,
        ];
        // A full stack of each resource, capped at that item's registry
        // stack limit so cloth (50), meteorite (20), and ancient fittings
        // (50) land as a legal max stack rather than an oversized one.
        const RESOURCE_TARGET_QUANTITY: u16 = 100;

        let mut placed = 0u32;
        let mut overflow = 0u32;

        // Equipables: actionbar first → inventory fallback. Each one
        // is a stack of 1 (tools and deployables are equipable), so
        // we never need to merge them with an existing matching stack.
        for item_id in EQUIPABLES {
            let stack = ItemStack::new(*item_id, 1);
            if let Some(slot) = client
                .inventory
                .actionbar_slots
                .iter()
                .position(Option::is_none)
            {
                client.inventory.actionbar_slots[slot] = Some(stack);
                placed += 1;
            } else if add_stack_to_inventory(&mut client.inventory, stack).is_some() {
                overflow += 1;
            } else {
                placed += 1;
            }
        }

        // Resources: inventory only. We clamp the target quantity to each
        // resource's stack limit (twine/wood/stone cap at 200, iron_bar /
        // sulfur / gunpowder at 100, cloth / ancient_fittings at 50,
        // meteorite at 20) so no slot ever holds an illegal oversized
        // stack. We pick the first empty inventory slot directly so
        // granting a kit doesn't merge into the player's existing piles in
        // unpredictable order.
        for item_id in RESOURCES {
            let quantity = stack_limit(item_id)
                .unwrap_or(RESOURCE_TARGET_QUANTITY)
                .min(RESOURCE_TARGET_QUANTITY);
            let stack = ItemStack::new(*item_id, quantity);
            if let Some(slot) = client
                .inventory
                .inventory_slots
                .iter()
                .position(Option::is_none)
            {
                client.inventory.inventory_slots[slot] = Some(stack);
                placed += 1;
            } else {
                overflow += 1;
            }
        }

        let message = if overflow == 0 {
            format!("test kit granted ({placed} items)")
        } else {
            format!(
                "test kit granted ({placed} items, {overflow} couldn't fit; clear some inventory)"
            )
        };
        reply_success(client_id, message)
    }

    /// `/give <item_id|all> [count]`, debug grant of raw materials.
    /// `all` hands out every base resource; a specific id hands out that
    /// item. Count defaults to 1000 and is split into registry-sized
    /// stacks. Admin only; whatever doesn't fit is reported, not lost
    /// silently.
    pub(super) fn command_give(
        &mut self,
        client_id: ClientId,
        args: &[&str],
    ) -> Vec<ServerEnvelope> {
        /// Every base resource `/give all` hands out.
        const ALL_RESOURCES: &[&str] = &[
            WOOD_ID,
            STONE_ID,
            COAL_ID,
            IRON_ORE_ID,
            SULFUR_ORE_ID,
            FIBER_ID,
            PLANT_TWINE_ID,
            IRON_BAR_ID,
            HEWN_LOG_ID,
        ];
        const DEFAULT_COUNT: u32 = 1000;
        const MAX_COUNT: u32 = 100_000;

        let Some(client) = self.clients.get(&client_id) else {
            return Vec::new();
        };
        if !client.is_admin {
            return reply_warning(client_id, "admin only");
        }
        let Some(&target) = args.first() else {
            return reply_warning(client_id, "usage: /give <item_id|all> [count]");
        };
        let count = match args.get(1) {
            None => DEFAULT_COUNT,
            Some(raw) => match raw.parse::<u32>() {
                Ok(count) if (1..=MAX_COUNT).contains(&count) => count,
                _ => {
                    return reply_warning(
                        client_id,
                        format!("count must be a number from 1 to {MAX_COUNT}"),
                    );
                }
            },
        };

        let definitions: Vec<&'static str> = if target.eq_ignore_ascii_case("all") {
            ALL_RESOURCES.to_vec()
        } else {
            match item_definition(target) {
                Some(definition) => vec![definition.id],
                None => {
                    return reply_warning(
                        client_id,
                        format!("unknown item: {target} (try /give all)"),
                    );
                }
            }
        };

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let mut granted: u64 = 0;
        let mut overflow: u64 = 0;
        for item_id in definitions {
            let stack_size = item_definition(item_id)
                .map(|definition| u32::from(definition.stack_size.max(1)))
                .unwrap_or(1);
            let mut remaining = count;
            while remaining > 0 {
                let quantity = remaining.min(stack_size).min(u32::from(u16::MAX)) as u16;
                let leftover = add_stack_to_inventory(
                    &mut client.inventory,
                    ItemStack::new(item_id, quantity),
                )
                .map(|stack| u32::from(stack.quantity))
                .unwrap_or(0);
                let added = u32::from(quantity) - leftover;
                granted += u64::from(added);
                remaining -= added;
                if leftover > 0 {
                    // Inventory is full; stop hammering this item. The
                    // unplaced rest (still in `remaining`) is reported.
                    break;
                }
            }
            overflow += u64::from(remaining);
        }
        let message = if overflow == 0 {
            format!("granted {granted} items")
        } else {
            format!("granted {granted} items ({overflow} couldn't fit; clear some inventory)")
        };
        reply_success(client_id, message)
    }
}
