use super::*;
use crate::protocol::{ItemStack, Vec3Net};

#[test]
fn pickup_target_clear_removes_cached_target() {
    let mut state = PickupTargetState {
        dropped_item_id: Some(7),
        stack: Some(ItemStack::new("ore", 1)),
        resource_node_id: Some(8),
        resource_definition_id: Some("node".to_owned()),
        resource_storage: vec![ItemStack::new("wood", 2)],
        world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
        screen_position: Some(Vec2::new(10.0, 20.0)),
        deployable_id: Some(42),
        deployable_kind: Some(crate::items::DeployableKind::Furnace { tier: 1 }),
        player_id: Some(99),
        loot_bag_id: Some(123),
        elapsed_since_scan: 0.0,
    };

    state.clear();

    assert!(state.dropped_item_id.is_none());
    assert!(state.stack.is_none());
    assert!(state.resource_node_id.is_none());
    assert!(state.resource_definition_id.is_none());
    assert!(state.resource_storage.is_empty());
    assert!(state.world_position.is_none());
    assert!(state.screen_position.is_none());
    assert!(state.player_id.is_none());
    assert!(state.loot_bag_id.is_none());
}

#[test]
fn gather_input_sends_at_swing_impact_and_repeats_while_held() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Axe;
    let duration = swing_duration_seconds(tool);
    let impact_time = duration * swing_impact_fraction(tool);

    let tick = state.update(
        0.01,
        true,
        true,
        Some(tool),
        Some(SwingTarget::ResourceNode(4)),
    );
    assert!(tick.is_none());
    assert!(state.swing_fraction() > 0.0);

    let impact = state
        .update(
            impact_time,
            false,
            true,
            Some(tool),
            Some(SwingTarget::ResourceNode(4)),
        )
        .expect("impact should emit at the impact fraction of the swing");
    assert_eq!(impact.target, Some(SwingTarget::ResourceNode(4)));
    assert_eq!(impact.tool, tool);

    // Same swing, no second impact even though we step further.
    assert!(
        state
            .update(
                0.01,
                false,
                true,
                Some(tool),
                Some(SwingTarget::ResourceNode(4))
            )
            .is_none()
    );

    let _ = state.update(
        duration,
        false,
        true,
        Some(tool),
        Some(SwingTarget::ResourceNode(5)),
    );
    // Swing rolled over into a new swing while LMB is held.
    assert!(state.swing_fraction() < 0.2);
}

#[test]
fn gather_input_emits_exactly_one_impact_event_at_impact_fraction() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Pickaxe;
    let duration = swing_duration_seconds(tool);
    let impact_time = duration * swing_impact_fraction(tool);

    // Up to one frame before the impact threshold: nothing fires.
    let pre_impact = impact_time - 0.001;
    assert!(
        state
            .update(
                pre_impact,
                true,
                true,
                Some(tool),
                Some(SwingTarget::ResourceNode(7))
            )
            .is_none()
    );

    // Crossing the impact threshold emits exactly one event.
    let impact = state
        .update(
            0.005,
            false,
            true,
            Some(tool),
            Some(SwingTarget::ResourceNode(7)),
        )
        .expect("impact should emit once we cross the impact fraction");
    assert_eq!(impact.target, Some(SwingTarget::ResourceNode(7)));
    assert_eq!(impact.tool, tool);

    // No duplicate impact for the remainder of the swing.
    assert!(
        state
            .update(
                duration * 0.1,
                false,
                false,
                Some(tool),
                Some(SwingTarget::ResourceNode(7))
            )
            .is_none()
    );
}

#[test]
fn gather_input_swings_without_target_and_yields_no_impact() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Pickaxe;
    let duration = swing_duration_seconds(tool);
    let impact_time = duration * swing_impact_fraction(tool);

    // Click with no target, swing still starts.
    let _ = state.update(0.01, true, true, Some(tool), None);
    assert!(state.swing_fraction() > 0.0);

    // Crossing the impact fraction emits a SwingImpact with no target.
    let impact = state
        .update(impact_time, false, true, Some(tool), None)
        .expect("impact frame should still fire");
    assert!(impact.target.is_none());
    assert_eq!(impact.tool, tool);
}

#[test]
fn gather_input_does_nothing_without_a_tool_equipped() {
    let mut state = GatherInputState::default();
    assert!(
        state
            .update(0.01, true, true, None, Some(SwingTarget::ResourceNode(4)))
            .is_none()
    );
    assert_eq!(state.swing_fraction(), 0.0);
}

#[test]
fn release_before_swing_completes_stops_after_one_impact() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Axe;
    let duration = swing_duration_seconds(tool);

    // Start a swing with a single click (just_pressed, not held).
    let _ = state.update(0.0, true, false, Some(tool), None);
    assert!(state.swing_fraction() >= 0.0);

    // Drive the whole swing with no further press. After it completes,
    // it must NOT roll into a new swing (pressed = false).
    let _ = state.update(duration, false, false, Some(tool), None);
    assert_eq!(
        state.swing_fraction(),
        0.0,
        "a released swing should end rather than repeat"
    );
}

#[test]
fn cancel_clears_active_swing_and_pending_feedback() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Pickaxe;
    let _ = state.update(0.01, true, true, Some(tool), None);
    state.set_pending_impact(PendingImpactEffect {
        anchor: Vec3::ZERO,
        spray_direction: Vec3::Y,
        kind: ImpactEffectKind::StoneShards,
        seed: 1,
    });
    state.set_pending_audio_cue(PendingAudioCue {
        anchor: Vec3::ZERO,
        tool,
        surface: SurfaceMaterial::Stone,
        is_player_hit: false,
    });
    state.set_pending_miss_audio();

    state.cancel();

    assert_eq!(state.swing_fraction(), 0.0);
    assert!(state.take_pending_impact().is_none());
    assert!(state.take_pending_audio_cue().is_none());
    assert!(!state.take_pending_miss_audio());
}

#[test]
fn pending_feedback_is_take_once() {
    let mut state = GatherInputState::default();
    state.set_pending_miss_audio();
    assert!(
        state.take_pending_miss_audio(),
        "first take returns the flag"
    );
    assert!(
        !state.take_pending_miss_audio(),
        "the flag is consumed on take"
    );

    let cue = PendingAudioCue {
        anchor: Vec3::new(1.0, 0.0, 0.0),
        tool: ToolKind::Hands,
        surface: SurfaceMaterial::Dirt,
        is_player_hit: true,
    };
    state.set_pending_audio_cue(cue);
    let taken = state.take_pending_audio_cue().expect("cue present");
    assert!(taken.is_player_hit);
    assert!(state.take_pending_audio_cue().is_none());
}

#[test]
fn each_started_swing_advances_the_seed() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Hands;
    let duration = swing_duration_seconds(tool);
    // Land each swing (real target) so it rolls straight into the next
    // while held; a whiff would charge the miss-recovery gap instead and
    // not roll over until it elapsed.
    let target = Some(SwingTarget::ResourceNode(4));

    let _ = state.update(0.0, true, true, Some(tool), target);
    let first = state.current_swing_seed();
    // Roll into a second swing by completing the first while held.
    let _ = state.update(duration, false, true, Some(tool), target);
    let second = state.current_swing_seed();
    assert_ne!(first, second, "each swing should bump the seed");
}

#[test]
fn missed_swing_locks_out_the_next_until_recovery_elapses() {
    use crate::game_balance::COMBAT_MISS_RECOVERY_SECONDS;

    let mut state = GatherInputState::default();
    let tool = ToolKind::Axe;
    let duration = swing_duration_seconds(tool);

    // Hold LMB through a full swing that connects with nothing.
    let _ = state.update(0.0, true, true, Some(tool), None);
    let impact = state
        .update(duration, false, true, Some(tool), None)
        .expect("the whiff still emits an impact event");
    assert!(impact.target.is_none());

    // The held button must NOT roll straight into a new swing: the miss
    // recovery gap is in effect, so the tool stays idle.
    let _ = state.update(0.0, false, true, Some(tool), None);
    assert_eq!(
        state.swing_fraction(),
        0.0,
        "a whiffed swing should not repeat until the recovery gap elapses"
    );

    // Still holding, but only part of the gap has passed: still locked.
    let _ = state.update(
        COMBAT_MISS_RECOVERY_SECONDS * 0.5,
        false,
        true,
        Some(tool),
        None,
    );
    assert_eq!(
        state.swing_fraction(),
        0.0,
        "half the recovery window is not enough to resume swinging"
    );

    // Once the rest of the gap elapses, the held button starts a new swing.
    let _ = state.update(COMBAT_MISS_RECOVERY_SECONDS, false, true, Some(tool), None);
    assert!(
        state.swing_fraction() > 0.0,
        "the swing resumes once the recovery gap has elapsed"
    );
}

#[test]
fn landed_swing_repeats_with_no_recovery_penalty() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Axe;
    let duration = swing_duration_seconds(tool);
    let target = Some(SwingTarget::Player(7));

    // A swing that lands on a player should roll straight into the next
    // one while LMB is held, no recovery gap, full cadence preserved.
    let _ = state.update(0.0, true, true, Some(tool), target);
    let _ = state.update(duration, false, true, Some(tool), target);
    assert!(
        state.swing_fraction() < 0.2,
        "a landed swing repeats immediately while held"
    );
}

#[test]
fn tool_swap_or_death_clears_a_pending_miss_recovery() {
    let mut state = GatherInputState::default();
    let tool = ToolKind::Axe;
    let duration = swing_duration_seconds(tool);

    // Whiff to arm the recovery gap.
    let _ = state.update(0.0, true, true, Some(tool), None);
    let _ = state.update(duration, false, true, Some(tool), None);

    // Cancel (tool swap / death) should wipe the lockout so the next tool
    // is usable immediately rather than inheriting a stun.
    state.cancel();
    let _ = state.update(0.01, true, true, Some(tool), Some(SwingTarget::Player(3)));
    assert!(
        state.swing_fraction() > 0.0,
        "after cancel a fresh click swings without waiting out the old recovery"
    );
}

#[test]
fn impact_effect_kind_maps_models_and_surfaces() {
    use crate::resources::ResourceNodeModel;
    assert_eq!(
        ImpactEffectKind::for_resource_model(ResourceNodeModel::PineTreeLarge),
        ImpactEffectKind::WoodChips
    );
    assert_eq!(
        ImpactEffectKind::for_resource_model(ResourceNodeModel::BranchPile),
        ImpactEffectKind::Sticks
    );
    assert_eq!(
        ImpactEffectKind::for_resource_model(ResourceNodeModel::SurfaceStone),
        ImpactEffectKind::Pebbles
    );
    assert_eq!(
        ImpactEffectKind::for_resource_model(ResourceNodeModel::HayGrass),
        ImpactEffectKind::GrassBlades
    );
    assert_eq!(
        ImpactEffectKind::for_resource_model(ResourceNodeModel::IronOre),
        ImpactEffectKind::StoneShards
    );

    assert_eq!(
        ImpactEffectKind::for_surface(SurfaceMaterial::Wood),
        ImpactEffectKind::WoodChips
    );
    assert_eq!(
        ImpactEffectKind::for_surface(SurfaceMaterial::Coal),
        ImpactEffectKind::StoneShards
    );
}

#[test]
fn impact_effect_kind_maps_wire_kinds() {
    use crate::protocol::ResourceImpactKind;
    assert_eq!(
        ImpactEffectKind::for_resource_impact(ResourceImpactKind::Tree),
        ImpactEffectKind::WoodChips
    );
    assert_eq!(
        ImpactEffectKind::for_resource_impact(ResourceImpactKind::SurfaceStone),
        ImpactEffectKind::Pebbles
    );
    assert_eq!(
        ImpactEffectKind::for_resource_impact(ResourceImpactKind::HayGrass),
        ImpactEffectKind::GrassBlades
    );
}

#[test]
fn swap_durations_scale_with_item_weight() {
    // Bag/deployable share the lightest lift; the pickaxe is heaviest.
    assert!(
        swap_duration_for_model(ItemModel::Pickaxe) > swap_duration_for_model(ItemModel::Hatchet)
    );
    assert!(swap_duration_for_model(ItemModel::Hatchet) > swap_duration_for_model(ItemModel::Bag));
    assert_eq!(
        swap_duration_for_model(ItemModel::Bag),
        swap_duration_for_model(ItemModel::Deployable)
    );
}

#[test]
fn tool_swap_state_resets_on_new_item_and_progresses_while_held() {
    let mut swap = ToolSwapState::default();
    // No item → fully settled, not swapping.
    swap.observe(0.1, None);
    assert_eq!(swap.fraction(), 1.0);
    assert!(!swap.is_swapping());

    // New item begins a swap at fraction 0.
    swap.observe(0.0, Some(("hatchet", ItemModel::Hatchet)));
    assert_eq!(swap.fraction(), 0.0);
    assert!(swap.is_swapping());

    // Advancing the same item progresses the animation.
    let dur = swap_duration_for_model(ItemModel::Hatchet);
    swap.observe(dur * 0.5, Some(("hatchet", ItemModel::Hatchet)));
    let mid = swap.fraction();
    assert!(
        mid > 0.0 && mid < 1.0,
        "expected partial progress, got {mid}"
    );

    // Finishing the duration settles it.
    swap.observe(dur, Some(("hatchet", ItemModel::Hatchet)));
    assert_eq!(swap.fraction(), 1.0);
    assert!(!swap.is_swapping());

    // Switching to a different item restarts the swap.
    swap.observe(0.0, Some(("pickaxe", ItemModel::Pickaxe)));
    assert_eq!(swap.fraction(), 0.0);
    assert!(swap.is_swapping());

    // Dropping to no item resets entirely.
    swap.observe(0.0, None);
    assert_eq!(swap.fraction(), 1.0);
}
