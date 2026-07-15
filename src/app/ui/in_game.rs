use bevy::prelude::*;
use bevy_egui::egui;

use super::super::audio::{PlaySound, SoundId};
use super::chat::chat_ui;
use super::crafting_queue::crafting_queue_hud;
use super::death_splash::{death_splash_ui, send_respawn};
use super::deployable_overlay::{
    DeployableOverlay, collect_deployable_overlay_entries, deployable_overlay_ui,
};
use super::floating_text::floating_damage_ui;
use super::furnace::furnace_ui;
use super::hud::hud_ui;
use super::inventory::{draw_drag_preview, handle_drag_release};
use super::inventory_panel::inventory_panel_ui;
use super::loot_bag::loot_bag_ui;
use super::options::{OptionsBackTarget, VoiceTabIo, options_ui};
use super::pause::pause_ui;
use super::peer_overlay::{PeerOverlay, collect_peer_overlay_entries, peer_overlay_ui};
use super::text_prompt::text_prompt_ui;
use super::toast::toast_ui;
use super::tutorial::{self, TutorialStep, tutorial_step, tutorial_ui};
use super::wheel::wheel_ui;
use super::workbench::workbench_ui;
use super::world_map::world_map_ui;
use super::{UiResources, world_ready_for_play};

/// Scans the player's in-AoI resource nodes for crude (hand-pickup) nodes,
/// pairing each with the item it yields.
///
/// Crude (hand-pickup) nodes only, paired with what they yield, so the gather
/// ring points at branches/stones/grass, never a tree or rock that needs a tool
/// the player doesn't have yet. Registry access (`crate::resources` /
/// `crate::items`) lives here next to the tutorial logic that consumes it.
fn nearby_crude_nodes(
    resource_nodes: &Query<&'static crate::server::ResourceNode>,
) -> Vec<(Vec3, &'static str)> {
    resource_nodes
        .iter()
        .filter_map(|node| {
            let definition = crate::resources::resource_node_definition(&node.definition_id)?;
            if definition.required_tool.kind != crate::items::ToolKind::Hands {
                return None;
            }
            let yield_item = definition.storage.first().map(|mat| mat.item_id)?;
            Some((
                Vec3::new(node.position.x, node.position.y, node.position.z),
                yield_item,
            ))
        })
        .collect()
}

/// Project the replicated deployable set down to the crafting stations the
/// panel cares about: those with a non-zero `station_radius` (workbenches
/// today). Keeping only real stations keeps the per-frame set small; the
/// proximity math itself lives in `crafting::stations` and mirrors the
/// server's `station_in_range`.
fn collect_nearby_stations(
    deployables: &Query<(
        &'static crate::server::Deployable,
        &'static crate::server::DeployableTransform,
    )>,
) -> Vec<super::crafting::NearbyStation> {
    deployables
        .iter()
        .filter_map(|(meta, transform)| {
            let definition = crate::items::item_definition(&meta.item_id)?;
            let profile = definition.deployable?;
            if profile.station_radius <= 0.0 {
                return None;
            }
            Some(super::crafting::NearbyStation::new(
                meta.kind,
                definition.id,
                transform.position,
            ))
        })
        .collect()
}

/// Cost label pinned under the building-placement ghost: the material and
/// amount the piece costs, coloured green when the player can pay and red (with
/// how much they currently hold) when they can't. The anchor is the ghost's
/// projected base, set by `update_placement_ghost_system`; the readout is
/// absent for deployables and doors, so this is a no-op then.
fn building_cost_overlay(
    ctx: &egui::Context,
    placement: &crate::app::state::DeployablePlacementState,
) {
    let Some(readout) = placement.building_cost else {
        return;
    };
    let (color, label) = if readout.affordable() {
        (
            egui::Color32::from_rgb(120, 230, 130),
            format!("{} {}", readout.required, readout.material),
        )
    } else {
        (
            egui::Color32::from_rgb(240, 110, 110),
            format!(
                "{} {} (have {})",
                readout.required, readout.material, readout.have
            ),
        )
    };
    egui::Area::new(egui::Id::new("building_cost_overlay"))
        .order(egui::Order::Foreground)
        .interactable(false)
        // Pin under the ghost's base and centre the label there.
        .fixed_pos(egui::pos2(readout.anchor.x, readout.anchor.y + 14.0))
        .pivot(egui::Align2::CENTER_TOP)
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgba_unmultiplied(4, 6, 12, 220))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(90, 108, 128, 120),
                ))
                .corner_radius(5)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(label).size(14.0).strong().color(color),
                        )
                        .selectable(false),
                    );
                });
        });
}

/// Renders the whole in-game UI stack for the `Screen::InGame` arm: HUD, peer
/// overlay, floating damage, deployable overlay, tutorial, inventory + crafting
/// panel, furnace, loot bag, drag release/preview, crafting queue HUD, chat,
/// toast, death splash, and pause. egui draw ordering is significant here, so
/// the sequence below is preserved verbatim from the dispatcher.
pub(super) fn in_game_ui(ctx: &egui::Context, resources: &mut UiResources, delta_seconds: f32) {
    if resources.menu.pause_options_open {
        let primary_monitor = resources.primary_monitor.single().ok();
        let mut voice_io = VoiceTabIo {
            devices: &resources.voice_devices,
            control: &mut resources.voice_control,
            input_level: resources.voice.mic_level(),
            playback_available: resources.voice.playback_available,
        };
        options_ui(
            ctx,
            &mut resources.menu,
            &mut resources.settings,
            &mut resources.options_ui,
            &resources.physical_keys,
            primary_monitor,
            OptionsBackTarget::PauseMenu,
            &mut voice_io,
        );
    } else {
        // Screenshot toggles. `show_hud` is the master switch for all
        // always-on HUD chrome; `show_chat` additionally hides just the
        // chat box. Neither pauses the game: the world keeps simulating,
        // these only gate what's painted on top.
        let show_hud = resources.settings.hud.show_hud;
        let show_chat = resources.settings.hud.show_chat;
        if show_hud {
            hud_ui(
                ctx,
                &resources.runtime,
                &resources.diagnostics,
                &resources.settings,
                &resources.voice,
                &resources.combat_feedback,
                &resources.local_player,
                &resources.ranged_input,
                &resources.throw_charge,
                &resources.consume_charge,
                // The actionbar rect is one frame stale (it is set while drawing
                // the actionbar, later in this same in-game pass), which is fine
                // for anchoring the ranged ammo count + reload bar off it, exactly
                // as chat.rs / toast.rs already consume it.
                resources.inventory_ui.actionbar_rect,
            );
        }
        // Suppress the peer overlay (nameplates, chat bubbles)
        // whenever a full-screen modal is up. Nameplates
        // render at Order::Foreground; without this gate
        // they'd poke through the bag / furnace / inventory /
        // crafting panels.
        let world_overlays_visible = !resources.menu.inventory_open
            && !resources.menu.crafting_open
            && !resources.menu.furnace_open
            && !resources.menu.loot_bag_open
            && !resources.menu.workbench_open
            // The world map covers the scene, so suppress nameplates, floating
            // damage, and structure labels under it.
            && !resources.menu.world_map_open;
        let camera = resources
            .peer_overlay
            .camera
            .single()
            .ok()
            .map(|(camera, transform)| (camera, *transform));
        if world_overlays_visible && show_hud {
            let peers = collect_peer_overlay_entries(
                resources.peer_overlay.network_players.iter(),
                resources.peer_overlay.replicated_players.iter(),
                resources.runtime.client_id,
                &resources.voice,
            );
            peer_overlay_ui(ctx, PeerOverlay { camera, peers });
        }

        // Floating damage + deployable nametags are also
        // world-overlay layers; suppress them under the same
        // gate so a full-screen modal isn't pocked with
        // floating numbers and structure labels.
        if world_overlays_visible && show_hud {
            floating_damage_ui(ctx, camera, resources.floating_damage.iter());

            let entries = collect_deployable_overlay_entries(
                camera.map(|(_, transform)| transform.translation()),
                resources.deployable_overlay.placed.iter(),
                resources.deployable_overlay.replicated.iter(),
            );
            deployable_overlay_ui(ctx, DeployableOverlay { camera, entries });

            // Cost label pinned under the building-placement ghost, so the
            // player sees what a piece costs and whether they can pay before
            // committing. Filled (and projected) by the placement ghost system.
            building_cost_overlay(ctx, &resources.placement);
        }

        // Toggle-to-view world map. Drawn over the scene (translucent
        // backdrop) with a grid, axis labels, the player's own markers, and a
        // facing arrow. Interactive: right-click adds a marker, clicking one
        // opens a name/delete popup. The texture + markers are fetched/uploaded
        // by the world-map systems; mutations go out from here.
        if resources.menu.world_map_open && show_hud {
            world_map_ui(
                ctx,
                &resources.world_map,
                &mut resources.world_map_ui,
                &mut resources.menu,
                &mut resources.runtime,
                &mut resources.error_toasts,
                camera.map(|(_, transform)| transform),
            );
        }

        // Compute the tutorial step before the panel so the crafting
        // list can pin the focused recipes to the top (keeps their
        // outlines on-screen instead of below the scroll fold). The
        // overlay itself is drawn after the panel. Gate the step
        // computation on `tutorial_active`: the deficit scan builds
        // per-frame maps and clones, and once onboarding is complete
        // the result is never looked at again.
        let tutorial_active = !resources.settings.onboarding.completed
            && show_hud
            && world_ready_for_play(resources)
            && !resources.menu.pause_open
            && resources.menu.death_splash.is_none();
        let tutorial = if tutorial_active {
            tutorial_step(
                resources
                    .local_player
                    .private
                    .as_ref()
                    .map(|p| &p.inventory),
                resources.local_player.private.as_ref().map(|p| &p.crafting),
                resources.menu.inventory_open,
                resources.menu.crafting_open,
            )
        } else {
            TutorialStep::Done
        };
        ctx.memory_mut(|mem| {
            mem.data.insert_temp(
                tutorial::pin_recipes_key(),
                tutorial_active && tutorial == TutorialStep::CraftTools,
            );
        });

        // Resolve which crafting stations the local player is standing near,
        // once per frame, from the replicated deployable set. The crafting
        // panel uses it to gate workbench-tier recipes exactly the way the
        // server does (see `crafting::stations`), so the tier requirement is
        // legible in the row and the Craft button reflects it.
        let station_context = super::crafting::StationContext::new(
            resources.runtime.local_player_position().map(Into::into),
            collect_nearby_stations(&resources.crafting_stations),
        );

        // Unified inventory + crafting panel: one fixed-size shell
        // with a tab bar. Replaces the two separate modals; the
        // toggle systems flip which tab is active.
        inventory_panel_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.local_player,
            &mut resources.inventory_ui,
            &mut resources.crafting_ui,
            &station_context,
            &resources.pickup_target,
            &mut resources.inventory_sound_requests,
            &mut resources.error_toasts,
            delta_seconds,
            show_hud,
        );

        // Draw the tutorial overlay (card + focus highlights). Runs after
        // the panel so the tab/recipe rects it outlines are already
        // stashed in egui memory this frame; `tutorial`/`tutorial_active`
        // were computed above (before the panel) for the recipe pinning.
        if tutorial_active && tutorial == TutorialStep::Done {
            resources.settings.onboarding.completed = true;
            // Celebrate: the same arrival sting as the menu reveal, plus a
            // completion banner timed off this moment.
            resources
                .play_sound
                .write(PlaySound::non_spatial(SoundId::WorldJoin));
            let now = ctx.input(|input| input.time);
            ctx.memory_mut(|mem| mem.data.insert_temp(tutorial::celebrate_key(), now));
        } else if tutorial_active {
            let inventory = resources
                .local_player
                .private
                .as_ref()
                .map(|p| &p.inventory);
            let crafting = resources.local_player.private.as_ref().map(|p| &p.crafting);
            let player_position = resources.runtime.local_player_position();
            let crude_nodes = nearby_crude_nodes(&resources.resource_nodes);
            tutorial_ui(
                ctx,
                tutorial,
                inventory,
                crafting,
                camera,
                &crude_nodes,
                player_position,
            );
        }

        // Completion banner, self-gated off the timestamp stamped above,
        // so it lingers for a few seconds after the tutorial finishes.
        if show_hud {
            tutorial::completion_banner(ctx);
        }

        furnace_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.local_player,
            &mut resources.inventory_ui,
            &mut resources.error_toasts,
        );
        // Drawn right after the furnace: same "press E on a station" family,
        // and the ordering keeps its scrim/panel layered consistently with the
        // furnace's. Gates controls only (via `workbench_open`), never
        // simulation. See docs/gameplay-gating.md.
        workbench_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.local_player,
            &mut resources.error_toasts,
        );
        loot_bag_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &resources.local_player,
            &mut resources.inventory_ui,
            &mut resources.error_toasts,
        );
        // Drag release + preview run after every slot-drawing
        // surface (inventory, furnace) so the release decision
        // sees `hovered_slot` and the panel rects populated by
        // *this* frame. Without this ordering, a drag inside
        // the inventory while the furnace is open releases on
        // a `None` hovered_slot and falls through to the
        // drop-on-ground branch.
        handle_drag_release(
            ctx,
            &resources.menu,
            &mut resources.runtime,
            &mut resources.prediction,
            &resources.local_player,
            &mut resources.inventory_ui,
            &mut resources.error_toasts,
        );
        draw_drag_preview(ctx, &resources.inventory_ui);
        // The queue HUD is always visible while jobs exist,
        // closing the crafting browser must not hide it, that
        // would defeat the point of the queue being persistent.
        // The HUD master toggle still hides it for screenshots.
        if show_hud {
            crafting_queue_hud(
                ctx,
                &mut resources.runtime,
                &resources.local_player,
                &mut resources.crafting_hud,
                &mut resources.error_toasts,
            );
        }
        let inventory_open = resources.menu.inventory_open;
        let actionbar_rect = resources.inventory_ui.actionbar_rect;
        // Chat is independent of the HUD master: hiding the HUD for a
        // clean screenshot can still leave chat up and usable if the
        // chat toggle stays on.
        if show_chat {
            chat_ui(
                ctx,
                &mut resources.menu,
                &mut resources.runtime,
                &mut resources.error_toasts,
                inventory_open,
                actionbar_rect,
            );
        }
        if show_hud {
            toast_ui(ctx, &resources.toasts, actionbar_rect);
        }
        // Radial wheel (building plan piece select, hammer actions, door
        // code, bag rename). Input lives in the wheel system; this only
        // paints whatever is open.
        wheel_ui(ctx, &resources.wheel);
        // Single-field text dialog (door codes, bag rename). Drawn above
        // the wheel so a wheel-spawned prompt lands on top.
        text_prompt_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &mut resources.error_toasts,
        );
        // Death splash sits above every other in-game UI but
        // below modal dialogs / loading splash. Renders only
        // while `menu.death_splash` is set (server flipped the
        // local player to Dead and the runtime stored the
        // killer name).
        if let Some(splash) = resources.menu.death_splash.clone()
            && let Some(choice) = death_splash_ui(ctx, &splash)
        {
            send_respawn(&mut resources.runtime, choice);
        }
    }
    if resources.menu.pause_open && !resources.menu.pause_options_open {
        pause_ui(
            ctx,
            &mut resources.menu,
            &mut resources.runtime,
            &mut resources.shutdown_tasks,
            &resources.store,
            &mut resources.pending_session_end,
            &mut resources.update,
        );
    }
}
