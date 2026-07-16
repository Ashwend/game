//! Tool Cupboard upkeep and decay: the periodic drain that charges every
//! claimed base's building pieces against the materials stocked in its
//! cupboard's storage grid, and the decay that eats pieces whose tier went
//! unpaid.
//!
//! The economics (constants in [`crate::game_balance`]): every
//! `UPKEEP_PERIOD_TICKS` (5 real minutes) each cupboard is billed
//! per claimed building piece, in the piece's tier material (sticks pieces
//! cost raw wood, hewn-wood pieces hewn logs, stone pieces stone), at
//! `UPKEEP_PER_DAY_*` per in-game day. Sub-integer period costs accumulate
//! in `CupboardState::upkeep_carry` so the long-run rate is exact. Each
//! material bucket pays all-or-nothing per period: a bucket the storage
//! cannot cover takes nothing (no materials burned for no cover), flags
//! `upkeep_unpaid`, and every piece of that tier loses
//! `UPKEEP_DECAY_PCT_PER_PERIOD` percent of max HP (destroyed at zero
//! through the normal spill + stability path). Restocking stops further
//! decay at the next period; lost HP stays lost until hammer-repaired.
//!
//! Scope rules:
//! - Only building pieces are billed (doors, boxes, furnaces ride free).
//! - Only CLAIMED pieces decay: a piece is billed by every cupboard whose
//!   claim footprint covers it. Overlapping same-owner claims therefore
//!   double-pay rather than a piece slipping through free; foreign overlaps
//!   cannot happen (placement inside a foreign margin is blocked).
//! - Unclaimed structures neither pay nor decay.
//! - The schedule is real-time (the `/time-speed` cheat does not change
//!   what a base costs per real hour), mirroring the meteor scheduler.

use crate::{
    building::{BuildingTier, claim_cells_cover},
    game_balance::{
        UPKEEP_DECAY_PCT_PER_PERIOD, UPKEEP_PER_DAY_HEWN_LOGS, UPKEEP_PER_DAY_STICKS_WOOD,
        UPKEEP_PER_DAY_STONE, UPKEEP_PERIOD_TICKS,
    },
    items::{DeployableKind, HEWN_LOG_ID, STONE_ID, WOOD_ID},
    protocol::{ContainerUpkeepInfo, DeployedEntityId, ItemStack, SERVER_TICK_RATE_HZ},
    world_time::REAL_SECONDS_PER_DAY,
};

use super::GameServer;

/// The three upkeep buckets, indexed by [`BuildingTier`] order.
pub(crate) const UPKEEP_MATERIALS: [&str; 3] = [WOOD_ID, HEWN_LOG_ID, STONE_ID];
const UPKEEP_PER_DAY: [f32; 3] = [
    UPKEEP_PER_DAY_STICKS_WOOD,
    UPKEEP_PER_DAY_HEWN_LOGS,
    UPKEEP_PER_DAY_STONE,
];

const fn tier_index(tier: BuildingTier) -> usize {
    match tier {
        BuildingTier::Sticks => 0,
        BuildingTier::HewnWood => 1,
        BuildingTier::Stone => 2,
    }
}

/// One cupboard's bill for a period: how many claimed pieces of each tier it
/// covers, and their ids (the decay targets when a bucket goes unpaid).
pub(crate) struct UpkeepBill {
    pub(crate) cupboard: DeployedEntityId,
    pub(crate) counts: [u32; 3],
    pieces: [Vec<DeployedEntityId>; 3],
}

/// Fraction of an in-game day one upkeep period spans.
fn period_day_fraction() -> f32 {
    UPKEEP_PERIOD_TICKS as f32 / (REAL_SECONDS_PER_DAY * SERVER_TICK_RATE_HZ)
}

/// Total of `item_id` across a container slot grid.
fn count_in_slots(slots: &[Option<ItemStack>], item_id: &str) -> u32 {
    slots
        .iter()
        .flatten()
        .filter(|stack| stack.item_id.as_ref() == item_id)
        .map(|stack| u32::from(stack.quantity))
        .sum()
}

/// Remove up to `quantity` of `item_id` from the grid (front to back). The
/// caller checks availability first; this just performs the take.
fn take_from_slots(slots: &mut [Option<ItemStack>], item_id: &str, quantity: u32) {
    let mut remaining = quantity;
    for slot in slots.iter_mut() {
        if remaining == 0 {
            break;
        }
        let Some(stack) = slot else {
            continue;
        };
        if stack.item_id.as_ref() != item_id {
            continue;
        }
        let take = remaining.min(u32::from(stack.quantity)) as u16;
        stack.quantity -= take;
        remaining -= u32::from(take);
        if stack.quantity == 0 {
            *slot = None;
        }
    }
}

impl GameServer {
    /// Aggregate every cupboard's claimed building pieces per tier. Reads
    /// the same margin-inflated `claim_footprints` cache the placement gate
    /// uses, so "what you claim is what you pay for".
    pub(crate) fn upkeep_bills(&self) -> Vec<UpkeepBill> {
        let mut bills: Vec<UpkeepBill> = Vec::new();
        for (&cupboard_id, cells) in &self.claim_footprints {
            let mut bill = UpkeepBill {
                cupboard: cupboard_id,
                counts: [0; 3],
                pieces: Default::default(),
            };
            for entity in self.deployed_entities.values() {
                let DeployableKind::Building { tier, .. } = entity.kind else {
                    continue;
                };
                if !claim_cells_cover(cells, entity.position) {
                    continue;
                }
                let index = tier_index(tier);
                bill.counts[index] += 1;
                bill.pieces[index].push(entity.id);
            }
            bills.push(bill);
        }
        bills
    }

    /// The periodic upkeep drain + decay pass. Called every server tick;
    /// no-ops between periods.
    pub(in crate::server) fn tick_upkeep(&mut self) {
        if self.tick == 0 || !self.tick.is_multiple_of(UPKEEP_PERIOD_TICKS) {
            return;
        }
        let fraction = period_day_fraction();
        let bills = self.upkeep_bills();

        // Charge pass: per cupboard, per material bucket.
        let mut decaying: Vec<DeployedEntityId> = Vec::new();
        for bill in &bills {
            let Some(entity) = self.deployed_entity_mut(bill.cupboard) else {
                continue;
            };
            let (Some(storage), Some(cupboard)) =
                (entity.storage.as_mut(), entity.cupboard.as_mut())
            else {
                continue;
            };
            for index in 0..3 {
                if bill.counts[index] == 0 {
                    cupboard.upkeep_carry[index] = 0.0;
                    cupboard.upkeep_unpaid[index] = false;
                    continue;
                }
                let owed = bill.counts[index] as f32 * UPKEEP_PER_DAY[index] * fraction
                    + cupboard.upkeep_carry[index];
                let take = owed.floor() as u32;
                if take == 0 {
                    // Nothing due yet this period; bank the fraction.
                    cupboard.upkeep_carry[index] = owed;
                    cupboard.upkeep_unpaid[index] = false;
                    continue;
                }
                if count_in_slots(&storage.slots, UPKEEP_MATERIALS[index]) >= take {
                    take_from_slots(&mut storage.slots, UPKEEP_MATERIALS[index], take);
                    cupboard.upkeep_carry[index] = owed - take as f32;
                    cupboard.upkeep_unpaid[index] = false;
                } else {
                    // Unpaid: take nothing, drop the debt (the punishment is
                    // the decay, not a compounding bill), decay the tier.
                    cupboard.upkeep_carry[index] = 0.0;
                    cupboard.upkeep_unpaid[index] = true;
                    decaying.extend(bill.pieces[index].iter().copied());
                }
            }
        }

        // Decay pass: unpaid pieces rot by a fraction of max HP; pieces that
        // reach zero fall through the normal destroy path (spill + stability
        // + claim-footprint rebuild).
        let mut destroyed: Vec<DeployedEntityId> = Vec::new();
        for id in decaying {
            let Some(entity) = self.deployed_entity_mut(id) else {
                continue;
            };
            let loss = (entity
                .max_health
                .saturating_mul(UPKEEP_DECAY_PCT_PER_PERIOD)
                / 100)
                .max(1);
            entity.health = entity.health.saturating_sub(loss);
            if entity.health == 0 {
                destroyed.push(id);
            }
        }
        for id in destroyed {
            self.destroy_deployed_entity(id);
        }
    }

    /// The upkeep readout the Tool Cupboard's container view carries: one
    /// row per material with a nonzero bill, plus the decay flag. `None`
    /// for non-cupboard entities.
    pub(crate) fn upkeep_info_for(&self, id: DeployedEntityId) -> Option<ContainerUpkeepInfo> {
        let entity = self.deployed_entities.get(&id)?;
        if !matches!(entity.kind, DeployableKind::ToolCupboard) {
            return None;
        }
        let storage = entity.storage.as_ref()?;
        let cupboard = entity.cupboard.as_ref()?;
        let counts = self
            .upkeep_bills()
            .into_iter()
            .find(|bill| bill.cupboard == id)
            .map(|bill| bill.counts)
            .unwrap_or([0; 3]);
        let materials = (0..3)
            .filter(|&index| counts[index] > 0)
            .map(|index| {
                let per_day = (counts[index] as f32 * UPKEEP_PER_DAY[index]).ceil() as u32;
                let stocked = count_in_slots(&storage.slots, UPKEEP_MATERIALS[index]);
                (
                    crate::items::intern_item_id(UPKEEP_MATERIALS[index]),
                    per_day,
                    stocked,
                )
            })
            .collect();
        Some(ContainerUpkeepInfo {
            materials,
            decaying: cupboard.upkeep_unpaid.iter().any(|unpaid| *unpaid),
        })
    }
}
