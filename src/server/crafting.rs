//! Server-authoritative crafting queue.
//!
//! Inputs are consumed when a job is enqueued. The queue advances one
//! "current job" per server tick — parallel crafting is intentionally not
//! supported: queueing four arrows runs them serially behind one progress
//! bar, which keeps the wire model trivial and the UI honest about wait
//! time.
//!
//! Outputs are granted into the player's inventory on completion. If the
//! result doesn't fit, it spawns as a dropped world item at the player's
//! feet — the same recovery the resource gather path uses. Cancels follow
//! the same refund-then-drop rule for inputs.

use crate::{
    crafting::{MAX_CRAFTING_QUEUE_LEN, RecipeDefinition, RecipeStation, recipe_definition},
    items::{intern_item_id, item_definition},
    protocol::{
        ClientId, CraftingCommand, CraftingJob, CraftingJobId, ItemStack, MAX_CRAFT_BATCH_SIZE,
        PlayerCraftingState, SERVER_TICK_RATE_HZ, ServerMessage, ToastKind, ToastMessage, Vec3Net,
    },
};

use super::{
    DeliveryTarget, GameServer, ServerClient, ServerEnvelope,
    inventory::{add_stack_to_inventory, take_items_from_inventory},
    movement::{drop_position, drop_velocity},
};

/// Convert a recipe's wall-clock duration into a tick count for a single
/// unit. Always at least one tick so a zero-duration recipe still appears
/// in the queue for a frame — the UI relies on the head job being visible
/// long enough for the player to see *something* happen.
pub(super) fn craft_total_ticks(recipe: &RecipeDefinition) -> u32 {
    let ticks = (recipe.craft_seconds * SERVER_TICK_RATE_HZ).round() as u32;
    ticks.max(1)
}

/// Scale a single-unit tick count by the batch quantity, saturating at
/// `u32::MAX` so an absurd quantity can't underflow the math. The actual
/// per-message clamp lives on `MAX_CRAFT_BATCH_SIZE` — this is the
/// belt-and-braces guard.
pub(super) fn batch_total_ticks(recipe: &RecipeDefinition, quantity: u16) -> u32 {
    craft_total_ticks(recipe).saturating_mul(quantity.max(1) as u32)
}

/// Multiply an input quantity by the batch size, saturating at
/// `u16::MAX`. We clamp the *requested* batch to `MAX_CRAFT_BATCH_SIZE` so
/// this rarely saturates in practice, but the explicit guard keeps a
/// recipe with a giant per-unit input from wrapping around.
fn batch_input_quantity(per_unit: u16, quantity: u16) -> u16 {
    let total = (per_unit as u32).saturating_mul(quantity.max(1) as u32);
    total.min(u16::MAX as u32) as u16
}

impl GameServer {
    pub(super) fn apply_crafting_command(
        &mut self,
        client_id: ClientId,
        command: CraftingCommand,
    ) -> Vec<ServerEnvelope> {
        match command {
            CraftingCommand::Enqueue {
                recipe_id,
                quantity,
            } => self.enqueue_craft(client_id, &recipe_id, quantity),
            CraftingCommand::Cancel { job_id } => self.cancel_craft(client_id, job_id),
        }
    }

    fn enqueue_craft(
        &mut self,
        client_id: ClientId,
        recipe_id: &str,
        requested_quantity: u16,
    ) -> Vec<ServerEnvelope> {
        let Some(recipe) = recipe_definition(recipe_id) else {
            return craft_toast(
                client_id,
                ToastKind::Error,
                format!("Unknown recipe: {recipe_id}"),
            );
        };

        // Clamp the requested batch before any input math: a stray 0 or a
        // hostile huge value would otherwise either let a zero-cost craft
        // through or overflow the per-input multiplication below. The
        // clamp uses the protocol-side cap so the UI's "+ button stops at
        // max" rule is enforced authoritatively as well.
        let quantity = requested_quantity.clamp(1, MAX_CRAFT_BATCH_SIZE);

        // Station gate — check before borrowing `clients` mutably so the
        // immutable scan inside `station_in_range` doesn't fight the
        // borrow checker.
        if !self.station_in_range(client_id, recipe.station) {
            let station_label = match recipe.station {
                RecipeStation::None => "a station",
                RecipeStation::Workbench { .. } => "a workbench",
            };
            return craft_toast(
                client_id,
                ToastKind::Warning,
                format!("You need to be near {station_label}"),
            );
        }

        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };

        if client.crafting.jobs.len() >= MAX_CRAFTING_QUEUE_LEN {
            return craft_toast(
                client_id,
                ToastKind::Warning,
                "Crafting queue is full".to_owned(),
            );
        }

        if !has_inputs(client, recipe, quantity) {
            return craft_toast(
                client_id,
                ToastKind::Warning,
                format!("Not enough materials for {}", recipe.name),
            );
        }

        // Inputs are taken now and not refunded if the client disappears
        // mid-craft. The refund path is `Cancel` (above) and disconnect
        // cleanup (see `cancel_all_jobs_for_disconnect`). Batched: take
        // `per_unit × quantity` once, not in a loop, so a partial failure
        // mode can't leave the inventory half-debited.
        for input in recipe.inputs {
            let needed = batch_input_quantity(input.quantity, quantity);
            let removed = take_items_from_inventory(&mut client.inventory, input.item_id, needed);
            debug_assert_eq!(
                removed, needed,
                "has_inputs gate should guarantee the take succeeds"
            );
            let _ = removed;
        }

        let job_id = client.next_craft_job_id;
        client.next_craft_job_id = client.next_craft_job_id.wrapping_add(1);
        let job = CraftingJob::new(job_id, recipe.id, batch_total_ticks(recipe, quantity), quantity);
        client.crafting.jobs.push(job);
        Vec::new()
    }

    fn cancel_craft(&mut self, client_id: ClientId, job_id: CraftingJobId) -> Vec<ServerEnvelope> {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return Vec::new();
        };
        let Some(index) = client
            .crafting
            .jobs
            .iter()
            .position(|job| job.job_id == job_id)
        else {
            return Vec::new();
        };
        let job = client.crafting.jobs.remove(index);
        let Some(recipe) = recipe_definition(&job.recipe_id) else {
            // Recipe was retired between enqueue and cancel — drop the job
            // silently rather than refund inputs the registry no longer
            // knows about. This shouldn't happen with the current
            // append-only registry; the guard is here so a future
            // hot-reload doesn't deadlock the queue.
            return Vec::new();
        };

        // Refund the full batch worth of inputs. add_stack_to_inventory
        // already handles stack-limit overflow per item, so a quantity
        // that would saturate `u16` still lands the bulk in the player's
        // bag and the excess at their feet.
        let mut overflow = Vec::new();
        for input in recipe.inputs {
            let stack = ItemStack {
                item_id: intern_item_id(input.item_id),
                quantity: batch_input_quantity(input.quantity, job.quantity),
            };
            if let Some(remainder) = add_stack_to_inventory(&mut client.inventory, stack) {
                overflow.push(remainder);
            }
        }

        let drop_origin = drop_origin_for(client);
        self.spawn_refund_drops(overflow, drop_origin);
        Vec::new()
    }

    /// Advance each player's head job by one tick. Completed jobs grant
    /// their output (overflow drops at the player's feet) and a success
    /// toast. Run once per `GameServer::tick`.
    pub(super) fn tick_crafting(&mut self) -> Vec<ServerEnvelope> {
        let mut envelopes = Vec::new();
        let client_ids: Vec<ClientId> = self.clients.keys().copied().collect();
        for client_id in client_ids {
            self.tick_client_crafting(client_id, &mut envelopes);
        }
        envelopes
    }

    fn tick_client_crafting(&mut self, client_id: ClientId, envelopes: &mut Vec<ServerEnvelope>) {
        // Pull the head job's outcome out under the borrow scope, then
        // grant the output + emit the toast outside it. Keeps the
        // borrow checker happy without cloning the whole client.
        let mut completed: Option<(&'static RecipeDefinition, u16)> = None;
        let mut drop_origin = None;
        {
            let Some(client) = self.clients.get_mut(&client_id) else {
                return;
            };
            let Some(head) = client.crafting.jobs.first_mut() else {
                return;
            };
            head.progress_ticks = head.progress_ticks.saturating_add(1);
            if head.progress_ticks >= head.total_ticks {
                if let Some(recipe) = recipe_definition(&head.recipe_id) {
                    completed = Some((recipe, head.quantity));
                    drop_origin = Some(drop_origin_for(client));
                }
                client.crafting.jobs.remove(0);
            }
        }

        if let (Some((recipe, quantity)), Some(drop_origin)) = (completed, drop_origin) {
            self.grant_craft_output(client_id, recipe, quantity, drop_origin, envelopes);
        }
    }

    fn grant_craft_output(
        &mut self,
        client_id: ClientId,
        recipe: &'static RecipeDefinition,
        quantity: u16,
        drop_origin: DropOrigin,
        envelopes: &mut Vec<ServerEnvelope>,
    ) {
        let total_output =
            (recipe.output_quantity as u32).saturating_mul(quantity.max(1) as u32);
        let mut overflow = Vec::new();
        if let Some(client) = self.clients.get_mut(&client_id) {
            // Output can exceed `u16::MAX` for huge batches of stackable
            // items. Hand it to the inventory in `u16`-sized chunks so a
            // single oversized stack doesn't get silently truncated.
            let mut remaining = total_output;
            while remaining > 0 {
                let chunk = remaining.min(u16::MAX as u32) as u16;
                remaining -= chunk as u32;
                let stack = ItemStack {
                    item_id: intern_item_id(recipe.output_item),
                    quantity: chunk,
                };
                if let Some(remainder) = add_stack_to_inventory(&mut client.inventory, stack) {
                    overflow.push(remainder);
                }
            }
        }
        let granted_message = match item_definition(recipe.output_item) {
            Some(definition) if total_output == 1 => {
                format!("Crafted {}", definition.name)
            }
            Some(definition) => format!("Crafted {} ×{}", definition.name, total_output),
            None => format!("Crafted {}", recipe.name),
        };
        envelopes.push(ServerEnvelope {
            target: DeliveryTarget::Client(client_id),
            message: ServerMessage::Toast(ToastMessage::new(ToastKind::Success, granted_message)),
        });
        self.spawn_refund_drops(overflow, drop_origin);
    }

    /// Spawn the leftover stacks as dropped world items. Used both for
    /// craft-cancel refunds and inventory-full craft completions. Sharing
    /// one helper keeps the recovery path consistent.
    fn spawn_refund_drops(&mut self, stacks: Vec<ItemStack>, origin: DropOrigin) {
        for stack in stacks {
            self.spawn_dropped_item(stack, origin.position, origin.velocity, origin.yaw);
        }
    }

    /// Refund every queued job's inputs back into the client's inventory
    /// before the disconnect path persists their state. Called from the
    /// disconnect handler so the player isn't billed for jobs that never
    /// completed. Overflow lands as dropped items at their feet.
    pub(super) fn cancel_all_jobs_for_disconnect(&mut self, client_id: ClientId) {
        let Some(client) = self.clients.get_mut(&client_id) else {
            return;
        };
        let jobs: Vec<CraftingJob> = std::mem::take(&mut client.crafting.jobs);
        if jobs.is_empty() {
            return;
        }
        let drop_origin = drop_origin_for(client);
        let mut overflow: Vec<ItemStack> = Vec::new();
        for job in jobs {
            let Some(recipe) = recipe_definition(&job.recipe_id) else {
                continue;
            };
            for input in recipe.inputs {
                let stack = ItemStack {
                    item_id: intern_item_id(input.item_id),
                    quantity: batch_input_quantity(input.quantity, job.quantity),
                };
                if let Some(remainder) = add_stack_to_inventory(&mut client.inventory, stack) {
                    overflow.push(remainder);
                }
            }
        }
        self.spawn_refund_drops(overflow, drop_origin);
    }
}

fn craft_toast(client_id: ClientId, kind: ToastKind, text: String) -> Vec<ServerEnvelope> {
    vec![ServerEnvelope {
        target: DeliveryTarget::Client(client_id),
        message: ServerMessage::Toast(ToastMessage::new(kind, text)),
    }]
}

/// Does the client's inventory currently hold every input the recipe needs
/// for a batch of `quantity` units? Walks the inventory + actionbar slots
/// once per input and sums matching stacks. Acceptable for the early game;
/// if the inventory grows to thousands of slots we'd cache an
/// `item_id → quantity` map.
fn has_inputs(client: &ServerClient, recipe: &RecipeDefinition, quantity: u16) -> bool {
    for input in recipe.inputs {
        let needed = batch_input_quantity(input.quantity, quantity);
        let available = count_item_in_inventory(client, input.item_id);
        if available < needed {
            return false;
        }
    }
    true
}

fn count_item_in_inventory(client: &ServerClient, item_id: &str) -> u16 {
    let mut total: u32 = 0;
    let inventory = &client.inventory;
    for slot in inventory
        .inventory_slots
        .iter()
        .chain(inventory.actionbar_slots.iter())
    {
        if let Some(stack) = slot
            && stack.item_id.as_ref() == item_id
        {
            total = total.saturating_add(stack.quantity as u32);
        }
    }
    total.min(u16::MAX as u32) as u16
}

#[derive(Debug, Clone, Copy)]
struct DropOrigin {
    position: Vec3Net,
    velocity: Vec3Net,
    yaw: f32,
}

fn drop_origin_for(client: &ServerClient) -> DropOrigin {
    DropOrigin {
        position: drop_position(&client.controller),
        velocity: drop_velocity(&client.controller),
        yaw: client.controller.yaw,
    }
}

/// Default-empty starting crafting queue. Mirrors
/// [`super::inventory::starting_inventory`] so the connection handshake
/// has a single seed function per per-player resource.
pub(super) fn starting_crafting_state() -> PlayerCraftingState {
    PlayerCraftingState::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crafting::{PLANT_TWINE_RECIPE_ID, STONE_HATCHET_RECIPE_ID},
        items::{BASIC_HATCHET_ID, FIBER_ID, PLANT_TWINE_ID, STONE_ID, WOOD_ID},
        protocol::{GAME_VERSION, ItemStack, PROTOCOL_VERSION},
        save::WorldSave,
        server::ServerSettings,
        steam::{AuthMode, offline_auth_token},
    };

    fn make_server() -> GameServer {
        GameServer::new(
            WorldSave::new("Test", Some(1)),
            ServerSettings {
                auth_mode: AuthMode::Offline,
                singleplayer_host: Some(1),
            },
        )
    }

    fn add_test_client(server: &mut GameServer) -> ClientId {
        server
            .connect(
                PROTOCOL_VERSION,
                Some(GAME_VERSION.to_owned()),
                1,
                "Tester".to_owned(),
                offline_auth_token(1),
            )
            .expect("connect ok")
            .0
    }

    fn give_items(server: &mut GameServer, client_id: ClientId, item_id: &str, quantity: u16) {
        let client = server.clients.get_mut(&client_id).expect("client exists");
        let stack = ItemStack::new(item_id, quantity);
        let leftover = add_stack_to_inventory(&mut client.inventory, stack);
        assert!(leftover.is_none(), "test setup should fit");
    }

    #[test]
    fn enqueue_consumes_inputs_and_creates_a_job() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 5);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 1,
            },
        );

        let client = server.clients.get(&client_id).expect("client");
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 2);
        assert_eq!(client.crafting.jobs.len(), 1);
        assert_eq!(client.crafting.jobs[0].progress_ticks, 0);
    }

    #[test]
    fn enqueue_rejected_when_inputs_missing() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 1);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 1,
            },
        );

        let client = server.clients.get(&client_id).expect("client");
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 1);
        assert!(client.crafting.jobs.is_empty());
    }

    #[test]
    fn cancel_refunds_inputs() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, WOOD_ID, 5);
        give_items(&mut server, client_id, STONE_ID, 5);
        give_items(&mut server, client_id, PLANT_TWINE_ID, 1);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: STONE_HATCHET_RECIPE_ID.to_owned(),
                quantity: 1,
            },
        );
        let job_id = server.clients[&client_id].crafting.jobs[0].job_id;
        server.apply_crafting_command(client_id, CraftingCommand::Cancel { job_id });

        let client = server.clients.get(&client_id).expect("client");
        assert!(client.crafting.jobs.is_empty());
        assert_eq!(count_item_in_inventory(client, WOOD_ID), 5);
        assert_eq!(count_item_in_inventory(client, STONE_ID), 5);
        assert_eq!(count_item_in_inventory(client, PLANT_TWINE_ID), 1);
    }

    #[test]
    fn tick_grants_output_when_progress_reaches_total() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, WOOD_ID, 5);
        give_items(&mut server, client_id, STONE_ID, 5);
        give_items(&mut server, client_id, PLANT_TWINE_ID, 1);
        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: STONE_HATCHET_RECIPE_ID.to_owned(),
                quantity: 1,
            },
        );

        let total = server.clients[&client_id].crafting.jobs[0].total_ticks;
        for _ in 0..total {
            let mut envelopes = Vec::new();
            server.tick_client_crafting(client_id, &mut envelopes);
        }

        let client = server.clients.get(&client_id).expect("client");
        assert!(client.crafting.jobs.is_empty());
        assert_eq!(count_item_in_inventory(client, BASIC_HATCHET_ID), 1);
    }

    #[test]
    fn enqueue_with_quantity_takes_inputs_and_scales_total_ticks() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 9);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 3,
            },
        );

        let client = server.clients.get(&client_id).expect("client");
        // 3 fiber per twine × 3 twine = 9 consumed.
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 0);
        assert_eq!(client.crafting.jobs.len(), 1);
        let job = &client.crafting.jobs[0];
        assert_eq!(job.quantity, 3);
        let recipe = recipe_definition(PLANT_TWINE_RECIPE_ID).expect("recipe");
        assert_eq!(job.total_ticks, craft_total_ticks(recipe) * 3);
    }

    #[test]
    fn enqueue_quantity_rejected_when_inputs_short_for_full_batch() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        // Enough for 2 twine (6 fiber), but the player asks for 3.
        give_items(&mut server, client_id, FIBER_ID, 6);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 3,
            },
        );

        let client = server.clients.get(&client_id).expect("client");
        // No partial debit — nothing was taken because the full batch
        // didn't fit.
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 6);
        assert!(client.crafting.jobs.is_empty());
    }

    #[test]
    fn batch_completion_grants_full_output_stack() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 12);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 4,
            },
        );

        let total = server.clients[&client_id].crafting.jobs[0].total_ticks;
        for _ in 0..total {
            let mut envelopes = Vec::new();
            server.tick_client_crafting(client_id, &mut envelopes);
        }

        let client = server.clients.get(&client_id).expect("client");
        assert!(client.crafting.jobs.is_empty());
        // 4 twine in one completion.
        assert_eq!(count_item_in_inventory(client, PLANT_TWINE_ID), 4);
    }

    #[test]
    fn cancel_refunds_full_batch_quantity() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 15);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 5,
            },
        );
        assert_eq!(
            count_item_in_inventory(&server.clients[&client_id], FIBER_ID),
            0
        );

        let job_id = server.clients[&client_id].crafting.jobs[0].job_id;
        server.apply_crafting_command(client_id, CraftingCommand::Cancel { job_id });

        let client = server.clients.get(&client_id).expect("client");
        assert!(client.crafting.jobs.is_empty());
        // Full 15 fiber refunded.
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 15);
    }

    #[test]
    fn enqueue_clamps_zero_quantity_to_one() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 5);

        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 0,
            },
        );

        let client = server.clients.get(&client_id).expect("client");
        // Treated as quantity = 1, so 3 fiber consumed and one job queued.
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 2);
        assert_eq!(client.crafting.jobs.len(), 1);
        assert_eq!(client.crafting.jobs[0].quantity, 1);
    }

    #[test]
    fn disconnect_refunds_queued_inputs() {
        let mut server = make_server();
        let client_id = add_test_client(&mut server);
        give_items(&mut server, client_id, FIBER_ID, 3);
        server.apply_crafting_command(
            client_id,
            CraftingCommand::Enqueue {
                recipe_id: PLANT_TWINE_RECIPE_ID.to_owned(),
                quantity: 1,
            },
        );
        assert_eq!(
            count_item_in_inventory(&server.clients[&client_id], FIBER_ID),
            0
        );

        server.cancel_all_jobs_for_disconnect(client_id);

        let client = server.clients.get(&client_id).expect("client");
        assert_eq!(count_item_in_inventory(client, FIBER_ID), 3);
        assert!(client.crafting.jobs.is_empty());
    }
}
