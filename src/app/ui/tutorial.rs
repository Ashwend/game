//! First-run contextual tutorial.
//!
//! A state machine that watches the player's progress and guides them through
//! their first goal: gather raw materials, open the inventory, switch to
//! crafting, and craft a stone pickaxe + hatchet. Each step shows a card and
//! highlights what to focus on, the nearest gatherable pickup, the Crafting tab,
//! or the tool recipes. Advisory only: it never blocks input and advances itself
//! as the player makes progress, so it can't get stuck.
//!
//! The logic is **chain-aware** (the stone tools need plant twine, which is
//! crafted from fiber, so the gather goal expands to the raw gatherables wood,
//! stone, and fiber) and **queue-aware** (a tool counts as handled once it's
//! crafted *or* queued, so scheduling a craft never reverts a step even though
//! the inputs were just spent).
//!
//! Completion persists via `ClientSettings.onboarding.completed` (re-armable from
//! the options screen). The panel widgets stash the rects the overlay needs in
//! egui temp memory, so nothing has to be threaded through the panel.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    crafting::{
        PLANT_TWINE_RECIPE_ID, RecipeDefinition, STONE_HATCHET_RECIPE_ID, STONE_PICKAXE_RECIPE_ID,
        recipe_definition, recipes_iter,
    },
    items::{BASIC_HATCHET_ID, BASIC_PICKAXE_ID, PLANT_TWINE_ID, item_definition},
    protocol::{PlayerCraftingState, PlayerInventoryState},
};

use super::theme;

/// Green accent shared by every tutorial element (card border, ring, outlines).
const ACCENT: egui::Color32 = egui::Color32::from_rgb(168, 196, 120);

/// The two starter tools the tutorial completes on, as output item ids.
const TUTORIAL_TOOLS: [&str; 2] = [BASIC_PICKAXE_ID, BASIC_HATCHET_ID];
/// Recipe ids that produce those tools, index-aligned with `TUTORIAL_TOOLS`.
const TOOL_RECIPES: [&str; 2] = [STONE_PICKAXE_RECIPE_ID, STONE_HATCHET_RECIPE_ID];
/// Recipe ids the crafting step outlines: the tools plus the plant-twine
/// prerequisite (only outlined while twine is still short).
const HIGHLIGHT_RECIPES: [&str; 3] = [
    PLANT_TWINE_RECIPE_ID,
    STONE_PICKAXE_RECIPE_ID,
    STONE_HATCHET_RECIPE_ID,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::ui) enum TutorialStep {
    Gather,
    OpenInventory,
    OpenCrafting,
    CraftTools,
    Done,
}

/// egui temp-memory key the Crafting tab stashes its rect under.
pub(in crate::app::ui) fn crafting_tab_rect_key() -> egui::Id {
    egui::Id::new("tutorial_crafting_tab_rect")
}

/// egui temp-memory key a highlighted recipe row stashes its rect under.
pub(in crate::app::ui) fn recipe_rect_key(recipe_id: &str) -> egui::Id {
    egui::Id::new(("tutorial_recipe_rect", recipe_id))
}

/// egui temp-memory key for whether the crafting list should pin the tutorial
/// recipes to the top this frame (set while the tutorial is on the craft step).
pub(in crate::app::ui) fn pin_recipes_key() -> egui::Id {
    egui::Id::new("tutorial_pin_recipes")
}

/// egui temp-memory key for the crafting scroll viewport rect, used to clip
/// recipe outlines so they never spill below the panel.
pub(in crate::app::ui) fn craft_viewport_key() -> egui::Id {
    egui::Id::new("tutorial_craft_viewport")
}

/// egui temp-memory key for the timestamp (egui time, seconds) at which the
/// tutorial was completed, used to time the celebration banner.
pub(in crate::app::ui) fn celebrate_key() -> egui::Id {
    egui::Id::new("tutorial_completed_at")
}

/// How long the "tutorial complete" banner stays up after finishing.
const CELEBRATE_SECONDS: f64 = 6.0;

/// Whether `recipe_id` is one of the recipes the tutorial may outline (so the
/// crafting row knows to stash its rect).
pub(in crate::app::ui) fn is_tutorial_recipe(recipe_id: &str) -> bool {
    HIGHLIGHT_RECIPES.contains(&recipe_id)
}

/// Compute the current tutorial step from game state. Pure, self-advancing, and
/// both chain- and queue-aware (see the module docs).
pub(in crate::app::ui) fn tutorial_step(
    inventory: Option<&PlayerInventoryState>,
    crafting: Option<&PlayerCraftingState>,
    inventory_open: bool,
    crafting_open: bool,
) -> TutorialStep {
    let Some(inventory) = inventory else {
        return TutorialStep::Gather;
    };
    let counts = available_counts(inventory, crafting);

    // A tool is handled once it's in the bag or already queued to craft.
    let all_tools_handled = TUTORIAL_TOOLS
        .iter()
        .all(|tool| counts.get(*tool).copied().unwrap_or(0) >= 1);
    if all_tools_handled {
        return TutorialStep::Done;
    }

    // Still missing a tool: gather until we have the raw materials to make
    // everything we still need (intermediates included).
    if !raw_deficit(&counts).is_empty() {
        return TutorialStep::Gather;
    }

    if crafting_open {
        TutorialStep::CraftTools
    } else if inventory_open {
        TutorialStep::OpenCrafting
    } else {
        TutorialStep::OpenInventory
    }
}

/// Items the player has on hand plus the outputs of anything already queued to
/// craft. Queued outputs are counted so scheduling a craft doesn't read as "lost
/// the materials" and bounce the tutorial back a step.
fn available_counts(
    inventory: &PlayerInventoryState,
    crafting: Option<&PlayerCraftingState>,
) -> HashMap<String, u32> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for stack in inventory
        .inventory_slots
        .iter()
        .chain(inventory.actionbar_slots.iter())
        .flatten()
    {
        *counts.entry(stack.item_id.as_ref().to_owned()).or_default() += stack.quantity as u32;
    }
    if let Some(crafting) = crafting {
        for job in &crafting.jobs {
            if let Some(recipe) = recipe_definition(job.recipe_id.as_ref()) {
                *counts.entry(recipe.output_item.to_owned()).or_default() +=
                    recipe.output_quantity as u32 * job.quantity as u32;
            }
        }
    }
    counts
}

/// The recipe (if any) that produces `item`, looked up by output rather than id
/// so the chain expansion works even where a recipe id differs from its output.
fn recipe_for_output(item: &str) -> Option<&'static RecipeDefinition> {
    recipes_iter().find(|recipe| recipe.output_item == item)
}

/// Raw (non-craftable) materials the player still needs to make every starter
/// tool they don't yet have or have queued, expanded through intermediate
/// crafts. Empty means "you can build everything from what you've got".
fn raw_deficit(counts: &HashMap<String, u32>) -> Vec<(String, u32)> {
    let mut working = counts.clone();
    let mut deficit: HashMap<String, u32> = HashMap::new();
    for (recipe_id, tool) in TOOL_RECIPES.iter().zip(TUTORIAL_TOOLS.iter()) {
        if working.get(*tool).copied().unwrap_or(0) >= 1 {
            continue;
        }
        allocate(&mut working, &mut deficit, recipe_id, 1, 0);
    }
    let mut out: Vec<(String, u32)> = deficit.into_iter().filter(|(_, qty)| *qty > 0).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Recursively reserve `qty` of `output` from `counts`, crafting intermediates
/// as needed and accumulating any shortfall of raw materials into `deficit`.
/// `recipe_id` is the recipe that makes `output`.
fn allocate(
    counts: &mut HashMap<String, u32>,
    deficit: &mut HashMap<String, u32>,
    recipe_id: &str,
    qty: u32,
    depth: u32,
) {
    let Some(recipe) = recipe_definition(recipe_id) else {
        return;
    };
    let output = recipe.output_item;
    let mut remaining = qty;

    // Spend what we already have of the output first.
    if let Some(have) = counts.get_mut(output) {
        let take = (*have).min(remaining);
        *have -= take;
        remaining -= take;
    }
    if remaining == 0 {
        return;
    }

    // Guard against a (future) cyclic recipe graph.
    if depth >= 8 {
        *deficit.entry(output.to_owned()).or_default() += remaining;
        return;
    }

    let batches = remaining.div_ceil((recipe.output_quantity as u32).max(1));
    for input in recipe.inputs {
        reserve_input(
            counts,
            deficit,
            input.item_id,
            input.quantity as u32 * batches,
            depth,
        );
    }
}

/// Reserve `qty` of an input item: spend what we have, then either craft it (if
/// there's a recipe) or record the raw shortfall.
fn reserve_input(
    counts: &mut HashMap<String, u32>,
    deficit: &mut HashMap<String, u32>,
    item: &str,
    qty: u32,
    depth: u32,
) {
    let mut remaining = qty;
    if let Some(have) = counts.get_mut(item) {
        let take = (*have).min(remaining);
        *have -= take;
        remaining -= take;
    }
    if remaining == 0 {
        return;
    }
    match recipe_for_output(item) {
        Some(recipe) => allocate(counts, deficit, recipe.id, remaining, depth + 1),
        None => *deficit.entry(item.to_owned()).or_default() += remaining,
    }
}

/// Draw the guidance for the current step. Outline rects come from egui temp
/// memory, stashed by the panel widgets earlier this frame.
pub(in crate::app::ui) fn tutorial_ui(
    ctx: &egui::Context,
    step: TutorialStep,
    inventory: Option<&PlayerInventoryState>,
    crafting: Option<&PlayerCraftingState>,
    camera: Option<(&Camera, GlobalTransform)>,
    crude_nodes: &[(Vec3, &'static str)],
    player_position: Option<Vec3>,
) {
    let counts = inventory.map(|inv| available_counts(inv, crafting));
    match step {
        TutorialStep::Gather => {
            let deficit = counts.as_ref().map(raw_deficit).unwrap_or_default();
            draw_card(ctx, "Gather your first materials", &gather_body(&deficit));
            gather_ring(ctx, camera, crude_nodes, player_position, &deficit);
        }
        TutorialStep::OpenInventory => {
            draw_card(ctx, "Open your inventory", "Press Tab to open your bag.");
        }
        TutorialStep::OpenCrafting => {
            draw_card(ctx, "Open crafting", "Click the highlighted Crafting tab.");
            if let Some(rect) = read_rect(ctx, crafting_tab_rect_key()) {
                focus_outline(ctx, rect);
            }
        }
        TutorialStep::CraftTools => {
            let counts_ref = counts.as_ref();
            let viewport = read_rect(ctx, craft_viewport_key());

            if counts_ref.is_some_and(twine_short) {
                // Plant twine is a prerequisite for both stone tools, so focus it
                // by itself first; the tools come into focus once twine is sorted.
                draw_card(
                    ctx,
                    "Craft your tools",
                    "Craft Plant Twine first. Your tools need it.",
                );
                outline_recipe_if_visible(ctx, PLANT_TWINE_RECIPE_ID, viewport);
            } else {
                // Twine handled: outline only the tools the player still needs, so
                // a tool that's already crafted (or queued) drops out of focus.
                let mut missing_names: Vec<&str> = Vec::new();
                let mut outline: Vec<&str> = Vec::new();
                for (recipe_id, tool) in TOOL_RECIPES.iter().zip(TUTORIAL_TOOLS.iter()) {
                    if counts_ref.is_some_and(|c| c.get(*tool).copied().unwrap_or(0) >= 1) {
                        continue;
                    }
                    outline.push(recipe_id);
                    if let Some(recipe) = recipe_definition(recipe_id) {
                        missing_names.push(recipe.name);
                    }
                }
                let tools_phrase = join_with_and(&missing_names);
                let body = if tools_phrase.is_empty() {
                    "Craft your tools.".to_owned()
                } else {
                    format!("Craft your {tools_phrase}.")
                };
                draw_card(ctx, "Craft your tools", &body);
                for recipe in outline {
                    outline_recipe_if_visible(ctx, recipe, viewport);
                }
            }
        }
        TutorialStep::Done => {}
    }
}

/// Whether the player is short on plant twine for the tools they still need.
fn twine_short(counts: &HashMap<String, u32>) -> bool {
    let mut needed = 0u32;
    for (recipe_id, tool) in TOOL_RECIPES.iter().zip(TUTORIAL_TOOLS.iter()) {
        if counts.get(*tool).copied().unwrap_or(0) >= 1 {
            continue;
        }
        if let Some(recipe) = recipe_definition(recipe_id) {
            needed += recipe
                .inputs
                .iter()
                .filter(|input| input.item_id == PLANT_TWINE_ID)
                .map(|input| input.quantity as u32)
                .sum::<u32>();
        }
    }
    counts.get(PLANT_TWINE_ID).copied().unwrap_or(0) < needed
}

fn read_rect(ctx: &egui::Context, id: egui::Id) -> Option<egui::Rect> {
    ctx.memory(|mem| mem.data.get_temp::<egui::Rect>(id))
}

/// Build the gather card body: a "pick this up" line plus a "need N more X" line
/// for each raw material still short.
fn gather_body(deficit: &[(String, u32)]) -> String {
    let mut lines =
        vec!["Walk up to grass, branches, or loose stones and press E to pick them up.".to_owned()];
    for (id, needed) in deficit {
        let name = item_definition(id).map(|def| def.name).unwrap_or(id);
        lines.push(format!("Need {needed} more {name}"));
    }
    lines.join("\n")
}

/// Instructional card anchored under the top-center of the screen.
fn draw_card(ctx: &egui::Context, title: &str, body: &str) {
    egui::Area::new("tutorial_card".into())
        .order(egui::Order::Foreground)
        .interactable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
        .show(ctx, |ui| {
            ui.set_max_width(460.0);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(8, 11, 16, 236))
                .stroke(egui::Stroke::new(1.0, ACCENT))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(460.0 - 32.0);
                    ui.label(
                        egui::RichText::new("GETTING STARTED")
                            .size(10.5)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new(title)
                            .size(16.0)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(body)
                            .size(13.0)
                            .color(theme::muted_text()),
                    );
                });
        });
}

/// Celebration banner shown for a few seconds after the tutorial completes.
/// Self-gating off the completion timestamp in egui memory, so it keeps showing
/// even though the tutorial step machine has gone quiet (settings completed).
pub(in crate::app::ui) fn completion_banner(ctx: &egui::Context) {
    let Some(started) = ctx.memory(|mem| mem.data.get_temp::<f64>(celebrate_key())) else {
        return;
    };
    let elapsed = ctx.input(|input| input.time) - started;
    if !(0.0..=CELEBRATE_SECONDS).contains(&elapsed) {
        return;
    }
    // Fade out over the final second.
    let fade = ((CELEBRATE_SECONDS - elapsed) / 1.0).clamp(0.0, 1.0) as f32;

    egui::Area::new("tutorial_complete".into())
        .order(egui::Order::Foreground)
        .interactable(false)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            ui.set_max_width(460.0);
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(8, 11, 16, 236))
                .stroke(egui::Stroke::new(1.0, ACCENT))
                .corner_radius(8)
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(460.0 - 32.0);
                    ui.label(
                        egui::RichText::new("TUTORIAL COMPLETE")
                            .size(10.5)
                            .strong()
                            .color(ACCENT),
                    );
                    ui.add_space(3.0);
                    ui.label(
                        egui::RichText::new("You're all set")
                            .size(16.0)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Explore Ashwend freely: gather, build, and survive.")
                            .size(13.0)
                            .color(theme::muted_text()),
                    );
                });
        });
    ctx.request_repaint();
}

/// Pulsing outline around a UI element the player should click. Drawn on a
/// foreground layer and never clipped, so it always reads as a complete
/// rectangle around the element.
fn focus_outline(ctx: &egui::Context, rect: egui::Rect) {
    let pulse = pulse(ctx);
    let alpha = (150.0 + pulse * 105.0) as u8;
    let color = egui::Color32::from_rgba_unmultiplied(168, 196, 120, alpha);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("tutorial_outline"),
    ));
    painter.rect_stroke(
        rect.expand(3.0 + pulse * 2.0),
        egui::CornerRadius::same(6),
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Outside,
    );
    ctx.request_repaint();
}

/// Outline a recipe row by its stashed rect, but only when the row is at least
/// partially within the crafting scroll viewport. That keeps a row scrolled
/// fully out of view from painting an outline outside the panel, while leaving
/// the outline itself unclipped (so it's never cut off) for visible rows. Since
/// the focused recipes are pinned to the top of the list, they sit fully in view.
fn outline_recipe_if_visible(ctx: &egui::Context, recipe_id: &str, viewport: Option<egui::Rect>) {
    let Some(rect) = read_rect(ctx, recipe_rect_key(recipe_id)) else {
        return;
    };
    if let Some(viewport) = viewport
        && (rect.max.y <= viewport.min.y || rect.min.y >= viewport.max.y)
    {
        return;
    }
    focus_outline(ctx, rect);
}

/// Join names into a readable phrase: `[]` → "", `[a]` → "a", `[a, b]` →
/// "a and b", otherwise an Oxford-comma list.
fn join_with_and(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            if let Some((last, rest)) = names.split_last() {
                format!("{}, and {last}", rest.join(", "))
            } else {
                String::new()
            }
        }
    }
}

/// Pulsing ring on the nearest gatherable pickup, preferring nodes that yield a
/// material the player still needs, screen-projected so it tracks the world.
fn gather_ring(
    ctx: &egui::Context,
    camera: Option<(&Camera, GlobalTransform)>,
    crude_nodes: &[(Vec3, &'static str)],
    player_position: Option<Vec3>,
    deficit: &[(String, u32)],
) {
    let (Some((camera, camera_transform)), Some(player)) = (camera, player_position) else {
        return;
    };
    if crude_nodes.is_empty() {
        return;
    }

    // Point at the nearest node that yields a material the player is still
    // short on (so it never rings a rock when you need wood). Only if no such
    // node is in range do we fall back to the nearest pickup of any kind.
    let nearest_dist = |slice: &[(Vec3, &'static str)]| {
        slice
            .iter()
            .min_by(|a, b| {
                a.0.distance_squared(player)
                    .total_cmp(&b.0.distance_squared(player))
            })
            .copied()
    };
    let wanted: Vec<(Vec3, &'static str)> = crude_nodes
        .iter()
        .copied()
        .filter(|(_, yield_item)| deficit.iter().any(|(id, _)| id == yield_item))
        .collect();
    let Some((position, _)) = nearest_dist(&wanted).or_else(|| nearest_dist(crude_nodes)) else {
        return;
    };

    let anchor = position + Vec3::Y * 0.9;
    let to_node = anchor - camera_transform.translation();
    if to_node.dot(camera_transform.forward().as_vec3()) <= 0.0 {
        return;
    }
    let Ok(screen) = camera.world_to_viewport(&camera_transform, anchor) else {
        return;
    };

    let pulse = pulse(ctx);
    let alpha = (150.0 + pulse * 105.0) as u8;
    let color = egui::Color32::from_rgba_unmultiplied(168, 196, 120, alpha);
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("tutorial_ring"),
    ));
    painter.circle_stroke(
        egui::pos2(screen.x, screen.y),
        24.0 + pulse * 7.0,
        egui::Stroke::new(2.5, color),
    );
    ctx.request_repaint();
}

/// Shared 0..1 pulse so the ring and outlines breathe in sync.
fn pulse(ctx: &egui::Context) -> f32 {
    let time = ctx.input(|input| input.time) as f32;
    0.5 + 0.5 * (time * std::f32::consts::TAU * 1.1).sin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        items::{FIBER_ID, STONE_ID, WOOD_ID},
        protocol::{CraftingJob, ItemStack},
    };

    fn with_items(items: &[(&str, u16)]) -> PlayerInventoryState {
        let mut inventory = PlayerInventoryState::empty();
        for (slot, (id, qty)) in items.iter().enumerate() {
            inventory.inventory_slots[slot] = Some(ItemStack::new(*id, *qty));
        }
        inventory
    }

    /// Enough raw wood + stone + fiber to craft both stone tools (twine and all).
    fn raw_for_both_tools() -> PlayerInventoryState {
        with_items(&[(WOOD_ID, 4), (STONE_ID, 5), (FIBER_ID, 6)])
    }

    #[test]
    fn no_inventory_starts_at_gather() {
        assert_eq!(
            tutorial_step(None, None, false, false),
            TutorialStep::Gather
        );
    }

    #[test]
    fn empty_bag_is_gather_and_lists_raw_materials() {
        let inventory = PlayerInventoryState::empty();
        assert_eq!(
            tutorial_step(Some(&inventory), None, false, false),
            TutorialStep::Gather
        );
        // The deficit expands plant twine to fiber, so fiber shows up, never the
        // un-gatherable plant twine.
        let counts = available_counts(&inventory, None);
        let deficit = raw_deficit(&counts);
        assert!(deficit.iter().any(|(id, _)| id == FIBER_ID));
        assert!(deficit.iter().all(|(id, _)| id != PLANT_TWINE_ID));
    }

    #[test]
    fn having_only_wood_and_stone_still_needs_fiber() {
        let inventory = with_items(&[(WOOD_ID, 10), (STONE_ID, 10)]);
        assert_eq!(
            tutorial_step(Some(&inventory), None, false, false),
            TutorialStep::Gather
        );
    }

    #[test]
    fn enough_raw_materials_advances_through_panel_steps() {
        let inventory = raw_for_both_tools();
        assert_eq!(
            tutorial_step(Some(&inventory), None, false, false),
            TutorialStep::OpenInventory
        );
        assert_eq!(
            tutorial_step(Some(&inventory), None, true, false),
            TutorialStep::OpenCrafting
        );
        assert_eq!(
            tutorial_step(Some(&inventory), None, false, true),
            TutorialStep::CraftTools
        );
    }

    #[test]
    fn queued_twine_does_not_revert_to_gather() {
        // Twine queued (fiber already spent), wood + stone in hand: the tutorial
        // should treat the chain as satisfiable and stay on a craft step.
        let inventory = with_items(&[(WOOD_ID, 4), (STONE_ID, 5)]);
        let crafting = PlayerCraftingState {
            jobs: vec![CraftingJob::new(
                crate::protocol::CraftingJobId(1),
                PLANT_TWINE_RECIPE_ID,
                60,
                2,
            )],
        };
        assert_eq!(
            tutorial_step(Some(&inventory), Some(&crafting), false, true),
            TutorialStep::CraftTools
        );
    }

    #[test]
    fn queuing_both_tools_completes_without_reverting() {
        // Inputs spent, both tools queued: should be Done, not bounced back.
        let inventory = PlayerInventoryState::empty();
        let crafting = PlayerCraftingState {
            jobs: vec![
                CraftingJob::new(
                    crate::protocol::CraftingJobId(1),
                    STONE_PICKAXE_RECIPE_ID,
                    60,
                    1,
                ),
                CraftingJob::new(
                    crate::protocol::CraftingJobId(2),
                    STONE_HATCHET_RECIPE_ID,
                    60,
                    1,
                ),
            ],
        };
        assert_eq!(
            tutorial_step(Some(&inventory), Some(&crafting), false, true),
            TutorialStep::Done
        );
    }

    #[test]
    fn having_both_tools_completes() {
        let inventory = with_items(&[(BASIC_PICKAXE_ID, 1), (BASIC_HATCHET_ID, 1)]);
        assert_eq!(
            tutorial_step(Some(&inventory), None, false, false),
            TutorialStep::Done
        );
    }
}
