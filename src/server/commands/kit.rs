//! `/test-kit` and `/give`, debug commands that grant items.

use crate::{
    items::{
        BASIC_HATCHET_ID, BASIC_PICKAXE_ID, BUILDING_PLAN_ID, COAL_ID, CRUDE_FURNACE_ID, FIBER_ID,
        HAMMER_ID, HEWN_LOG_DOOR_ID, HEWN_LOG_ID, IRON_BAR_ID, IRON_HATCHET_ID, IRON_ORE_ID,
        IRON_PICKAXE_ID, PLANT_TWINE_ID, SLEEPING_BAG_ID, STICKS_ID, STONE_ID, SULFUR_ORE_ID,
        WOOD_ID, WORKBENCH_T1_ID, item_definition,
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

        // (item_id, quantity) tuples. Tools + deployables are equipables
        // and go to the actionbar first; resources go straight to the
        // inventory grid.
        const EQUIPABLES: &[&str] = &[
            BASIC_HATCHET_ID,
            BASIC_PICKAXE_ID,
            IRON_HATCHET_ID,
            IRON_PICKAXE_ID,
            WORKBENCH_T1_ID,
            CRUDE_FURNACE_ID,
            BUILDING_PLAN_ID,
            HAMMER_ID,
            HEWN_LOG_DOOR_ID,
            SLEEPING_BAG_ID,
        ];
        // Sticks appear twice on purpose: a starter base (foundation +
        // four wall pieces) costs more than one 100-stack.
        const RESOURCES: &[&str] = &[
            WOOD_ID,
            STONE_ID,
            COAL_ID,
            IRON_ORE_ID,
            SULFUR_ORE_ID,
            FIBER_ID,
            PLANT_TWINE_ID,
            IRON_BAR_ID,
            HEWN_LOG_ID,
            STICKS_ID,
            STICKS_ID,
        ];
        const RESOURCE_QUANTITY: u16 = 100;

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

        // Resources: inventory only. Stack of 100 fits inside every
        // resource's stack limit (twine/wood/stone/etc cap at 200,
        // iron_bar caps at 100). We pick the first empty inventory
        // slot directly so granting a kit doesn't merge into the
        // player's existing piles in unpredictable order.
        for item_id in RESOURCES {
            let stack = ItemStack::new(*item_id, RESOURCE_QUANTITY);
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
            STICKS_ID,
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
