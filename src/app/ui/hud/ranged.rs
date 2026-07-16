//! Ranged/throw/consume readouts: the ammo count tucked into the actionbar
//! corner plus the shared draw/reload/charge progress bar above it. The bow
//! draw, crossbow reload, thrown-bomb charge, and consumable use charge all
//! resolve to the same [`RangedHudView`] and paint through [`ranged_hud`].

use bevy_egui::egui;

use crate::{
    app::state::{ConsumeChargeState, LocalPlayerState, RangedDrawState, ThrowChargeState},
    inventory::count_items_in_inventory,
    items::item_definition,
};

/// Height of the draw/reload progress bar that sits just above the actionbar, in
/// px. A slim status readout, not a focal element, but tall enough to read at a
/// glance from across the screen (the earlier 5 px bar was too thin to see).
const RANGED_BAR_HEIGHT: f32 = 8.0;
/// Vertical gap between the top of the actionbar and the bottom of the ranged
/// progress bar, in px, so the bar floats clear of the actionbar frame.
const RANGED_BAR_GAP: f32 = 6.0;
/// Inset of the ammo count from the actionbar's lower-right corner, in px. Places
/// the small count inside the bottom-right of the actionbar frame where it reads
/// at a glance without covering a slot.
const RANGED_AMMO_INSET: egui::Vec2 = egui::vec2(6.0, 4.0);

/// What the ranged progress bar is filling with: a bow draw ramping to full, or a
/// crossbow reload cranking back to ready.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RangedHudFill {
    Draw(f32),
    Reload(f32),
}

/// Resolved per-frame inputs for the ranged HUD: the arrow count and (while a draw
/// or reload is live) the progress-bar fill. `None` whenever the active item is
/// not a ranged weapon, which is what keeps the HUD silent for melee / tools /
/// bare hands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RangedHudView {
    ammo: u32,
    fill: Option<RangedHudFill>,
}

/// Resolve the ranged HUD view off the active item + draw state. `None` unless a
/// ranged weapon is the active actionbar item.
pub(super) fn ranged_hud_view(
    local_player: &LocalPlayerState,
    ranged: &RangedDrawState,
) -> Option<RangedHudView> {
    let private = local_player.private.as_ref()?;
    let profile = private
        .inventory
        .active_actionbar_stack()
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.ranged)?;
    let ammo = count_items_in_inventory(&private.inventory, profile.ammo_item);
    let fill = if ranged.is_drawing() {
        Some(RangedHudFill::Draw(ranged.draw_fraction()))
    } else if ranged.is_reloading() {
        Some(RangedHudFill::Reload(ranged.reload_fraction()))
    } else {
        None
    };
    Some(RangedHudView { ammo, fill })
}

/// Resolve the thrown-bomb charge HUD view: the held bomb count plus (while a
/// charge is held) the ranged draw bar filling with the charge fraction. `None`
/// unless the active item is a thrown explosive AND a charge is live, so the
/// bar stays silent while just carrying a bomb.
pub(super) fn throw_hud_view(
    local_player: &LocalPlayerState,
    throw_charge: &ThrowChargeState,
) -> Option<RangedHudView> {
    if !throw_charge.is_charging() {
        return None;
    }
    let private = local_player.private.as_ref()?;
    let stack = private.inventory.active_actionbar_stack()?;
    let explosive = item_definition(&stack.item_id).and_then(|def| def.explosive)?;
    if explosive.delivery != crate::items::ExplosiveDelivery::Thrown {
        return None;
    }
    Some(RangedHudView {
        ammo: count_items_in_inventory(&private.inventory, &stack.item_id),
        fill: Some(RangedHudFill::Draw(throw_charge.charge_fraction())),
    })
}

/// Resolve the consumable use-charge HUD view: the held bandage count plus (while
/// a use is being held) the ranged draw bar filling with the use fraction. `None`
/// unless the active item is a consumable AND a use is live, so the bar stays
/// silent while just carrying bandages around.
///
/// The bar filling to full is a PREDICTION, not a completion: the server runs its
/// own charge clock and decides. The player will see the bandage leave their
/// actionbar and their health climb a frame or two after the bar tops out.
pub(super) fn consume_hud_view(
    local_player: &LocalPlayerState,
    consume: &ConsumeChargeState,
) -> Option<RangedHudView> {
    if !consume.is_using() {
        return None;
    }
    let private = local_player.private.as_ref()?;
    let stack = private.inventory.active_actionbar_stack()?;
    item_definition(&stack.item_id).and_then(|definition| definition.consumable)?;
    Some(RangedHudView {
        ammo: count_items_in_inventory(&private.inventory, &stack.item_id),
        fill: Some(RangedHudFill::Draw(consume.use_fraction())),
    })
}

/// The actionbar-anchored ranged readout: a small ammo count tucked into the
/// bottom-right of the actionbar, and (only while drawing / reloading) a
/// semi-transparent horizontal progress bar sitting just above the actionbar that
/// fills left-to-right with the draw fraction (bow) or reload progress (crossbow).
/// Both anchor off `actionbar_rect` (one frame stale, which is fine). Painted on a
/// dedicated foreground layer like the hit marker so no Area clips it. When the
/// actionbar rect isn't known yet (pre-first-frame), the HUD stays silent rather
/// than falling back to the crosshair.
pub(super) fn ranged_hud(
    ctx: &egui::Context,
    view: &RangedHudView,
    actionbar_rect: Option<egui::Rect>,
) {
    let Some(bar) = actionbar_rect else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("ranged_hud"),
    ));

    // Ammo count: small, monospace, muted; warms to red when the quiver is empty
    // so "why won't it fire" answers itself. Anchored to the actionbar's
    // bottom-right corner, inset so it reads as a corner label.
    let ammo_color = if view.ammo == 0 {
        egui::Color32::from_rgba_unmultiplied(235, 110, 90, 235)
    } else {
        egui::Color32::from_rgba_unmultiplied(230, 233, 238, 200)
    };
    painter.text(
        bar.right_bottom() - RANGED_AMMO_INSET,
        egui::Align2::RIGHT_BOTTOM,
        format!("{}", view.ammo),
        egui::FontId::monospace(14.0),
        ammo_color,
    );

    // Draw / reload progress bar: a translucent track plus the filled sweep,
    // spanning the actionbar width and sitting just above it. Only painted while a
    // draw or reload is live, so it is quiet during idle aim.
    let Some(fill) = view.fill else {
        return;
    };
    let (fraction, color) = match fill {
        // Draw charge: brightens as it fills so full draw reads at a glance. Warm
        // bright cord that goes near-opaque at full draw.
        RangedHudFill::Draw(f) => {
            let f = f.clamp(0.0, 1.0);
            let alpha = (170.0 + 80.0 * f) as u8;
            (
                f,
                egui::Color32::from_rgba_unmultiplied(240, 242, 245, alpha),
            )
        }
        // Reload progress: a cool "busy" fill, but bright enough to read clearly
        // against a lit world (the earlier dim value washed out on close inspection).
        RangedHudFill::Reload(f) => (
            f.clamp(0.0, 1.0),
            egui::Color32::from_rgba_unmultiplied(150, 200, 255, 230),
        ),
    };
    let (track_rect, fill_rect) = ranged_bar_rects(bar, fraction);
    // A darker opaque backing under the whole track so the fill reads with real
    // contrast against any world colour behind it, not just a faint tint.
    painter.rect_filled(
        track_rect,
        2.0,
        egui::Color32::from_rgba_unmultiplied(12, 16, 22, 200),
    );
    if fill_rect.width() > 0.0 {
        painter.rect_filled(fill_rect, 2.0, color);
    }
    // A thin light frame around the track so its extent is legible even at a low
    // fill fraction.
    painter.rect_stroke(
        track_rect,
        2.0,
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(230, 233, 238, 90),
        ),
        egui::StrokeKind::Inside,
    );
    // Keep the bar animating while it is live.
    ctx.request_repaint();
}

/// Geometry for the ranged progress bar: the full-width translucent track and the
/// left-to-right fill, both sitting `RANGED_BAR_GAP` above `actionbar` and
/// spanning its width. Pure so the placement is unit-testable without egui state.
fn ranged_bar_rects(actionbar: egui::Rect, fraction: f32) -> (egui::Rect, egui::Rect) {
    let fraction = fraction.clamp(0.0, 1.0);
    let bottom = actionbar.top() - RANGED_BAR_GAP;
    let top = bottom - RANGED_BAR_HEIGHT;
    let track = egui::Rect::from_min_max(
        egui::pos2(actionbar.left(), top),
        egui::pos2(actionbar.right(), bottom),
    );
    let fill = egui::Rect::from_min_max(
        track.min,
        egui::pos2(track.left() + track.width() * fraction, track.bottom()),
    );
    (track, fill)
}

#[cfg(test)]
mod tests {
    use super::super::raw_input;
    use super::*;

    /// A `LocalPlayerState` whose active actionbar item is `item_id`, with
    /// `arrows` arrows in the bag, for the ranged-HUD resolver tests.
    fn local_player_holding(item_id: &str, arrows: u16) -> LocalPlayerState {
        use crate::protocol::{ItemStack, PlayerInventoryState};
        let mut inventory = PlayerInventoryState::empty();
        inventory.actionbar_slots[0] = Some(ItemStack::new(item_id, 1));
        if arrows > 0 {
            inventory.inventory_slots[0] = Some(ItemStack::new("arrow", arrows));
        }
        LocalPlayerState {
            entity: None,
            private: Some(crate::server::PlayerPrivate {
                inventory,
                crafting: crate::protocol::PlayerCraftingState::default(),
                open_furnace: None,
                open_loot_bag: None,
                open_workbench: None,
                last_processed_input: 0,
                applied_action_seq: 0,
                run_speed_multiplier: 1.0,
                claim_status: Default::default(),
            }),
            lifecycle: None,
        }
    }

    #[test]
    fn ranged_hud_is_silent_for_melee_or_empty_hands() {
        // No private yet (pre-connect) and a melee tool both resolve to no view,
        // which is what keeps the HUD dark for everything that isn't a bow.
        let ranged = RangedDrawState::default();
        assert!(ranged_hud_view(&LocalPlayerState::default(), &ranged).is_none());
        assert!(
            ranged_hud_view(&local_player_holding("stone_hatchet", 5), &ranged).is_none(),
            "a melee tool never shows the ranged HUD"
        );
    }

    #[test]
    fn ranged_hud_view_reports_ammo_and_draw_fill() {
        // Holding a bow with 5 arrows: the view carries the count; idle shows no
        // bar fill; a held draw fills the bar with the draw fraction.
        let local = local_player_holding("wooden_bow", 5);
        let mut ranged = RangedDrawState::default();
        let idle = ranged_hud_view(&local, &ranged).expect("a held bow shows the HUD");
        assert_eq!(idle.ammo, 5);
        assert_eq!(idle.fill, None, "no bar fill while not drawing");

        // Start + hold a draw: the fill tracks the draw fraction.
        let profile = crate::items::item_definition("wooden_bow")
            .and_then(|d| d.ranged)
            .expect("wooden_bow has a ranged profile");
        let _ = ranged.update(0.0, true, true, Some(profile), true);
        let _ = ranged.update(0.5, false, true, Some(profile), true);
        let drawn = ranged_hud_view(&local, &ranged).expect("still holding the bow");
        match drawn.fill {
            Some(RangedHudFill::Draw(f)) => assert!(f > 0.0, "the bar fills with the draw"),
            other => panic!("expected a draw fill, got {other:?}"),
        }
    }

    #[test]
    fn ranged_hud_view_shows_reload_progress_for_the_crossbow() {
        let local = local_player_holding("crossbow", 3);
        let mut ranged = RangedDrawState::default();
        let profile = crate::items::item_definition("crossbow")
            .and_then(|d| d.ranged)
            .expect("crossbow has a ranged profile");
        // Fire and arm the reload the way the input layer does.
        let _ = ranged.update(0.0, true, true, Some(profile), true);
        ranged.begin_reload(profile);
        let _ = ranged.update(0.5, false, false, Some(profile), true);
        let view = ranged_hud_view(&local, &ranged).expect("crossbow shows the HUD");
        match view.fill {
            Some(RangedHudFill::Reload(f)) => {
                assert!(f > 0.0 && f < 1.0, "the bar tracks the reload, got {f}");
            }
            other => panic!("expected a reload fill, got {other:?}"),
        }
    }

    #[test]
    fn ranged_hud_paints_the_count_and_bar_when_the_actionbar_rect_is_known() {
        // A view with a live draw fill + a known actionbar rect paints shapes (the
        // ammo count text + the progress bar); this pins the painter path against
        // silently drawing nothing.
        let actionbar =
            egui::Rect::from_min_size(egui::pos2(300.0, 540.0), egui::vec2(200.0, 44.0));
        let ctx = egui::Context::default();
        let output = ctx.run_ui(raw_input(), |ui| {
            ranged_hud(
                ui.ctx(),
                &RangedHudView {
                    ammo: 12,
                    fill: Some(RangedHudFill::Draw(0.6)),
                },
                Some(actionbar),
            );
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn ranged_hud_is_silent_without_a_known_actionbar_rect() {
        // Before the actionbar has been laid out (its rect is None), the ranged
        // HUD paints nothing rather than falling back to the crosshair.
        let ctx = egui::Context::default();
        let output = ctx.run_ui(raw_input(), |ui| {
            ranged_hud(
                ui.ctx(),
                &RangedHudView {
                    ammo: 12,
                    fill: Some(RangedHudFill::Draw(0.6)),
                },
                None,
            );
        });
        assert!(output.shapes.is_empty());
    }

    #[test]
    fn ranged_bar_sits_above_the_actionbar_and_fills_left_to_right() {
        let actionbar =
            egui::Rect::from_min_size(egui::pos2(300.0, 540.0), egui::vec2(200.0, 44.0));
        // Empty fill: the track spans the full actionbar width; the fill is
        // zero-width. Both sit entirely above the actionbar.
        let (track, fill) = ranged_bar_rects(actionbar, 0.0);
        assert!(
            track.bottom() <= actionbar.top(),
            "the bar sits above the actionbar"
        );
        assert!((track.left() - actionbar.left()).abs() < 1e-4);
        assert!((track.right() - actionbar.right()).abs() < 1e-4);
        assert!(fill.width() < 1e-4, "an empty draw has no fill");

        // Half fill: the fill covers the left half of the track and shares its top.
        let (track, fill) = ranged_bar_rects(actionbar, 0.5);
        assert!((fill.width() - track.width() * 0.5).abs() < 1e-3);
        assert!(
            (fill.left() - track.left()).abs() < 1e-4,
            "fills from the left"
        );

        // Full fill covers the whole track; over-unity fractions clamp.
        let (track, fill) = ranged_bar_rects(actionbar, 2.0);
        assert!((fill.width() - track.width()).abs() < 1e-3);
    }
}
