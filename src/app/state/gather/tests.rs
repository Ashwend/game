use super::*;
use crate::protocol::{ItemStack, Vec3Net};

#[test]
fn pickup_target_clear_removes_cached_target() {
    let mut state = PickupTargetState {
        dropped_item_id: Some(crate::protocol::DroppedItemId(7)),
        stack: Some(ItemStack::new("ore", 1)),
        resource_node_id: Some(crate::protocol::ResourceNodeId(8)),
        resource_definition_id: Some("node".to_owned()),
        resource_storage: vec![ItemStack::new("wood", 2)],
        world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
        screen_position: Some(Vec2::new(10.0, 20.0)),
        deployable_id: Some(crate::protocol::DeployedEntityId(42)),
        deployable_kind: Some(crate::items::DeployableKind::Furnace { tier: 1 }),
        deployable_stability: Some(100),
        deployable_cupboard_auth: None,
        deployable_can_modify: false,
        deployable_demolishable: false,
        player_id: Some(crate::protocol::ClientId(99)),
        loot_bag_id: Some(crate::protocol::LootBagId(123)),
        sleeping_player: Some(("Sleeper".to_owned(), 50.0)),
        projectile_id: Some(crate::protocol::ProjectileId(31)),
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
    assert!(state.sleeping_player.is_none());
    assert!(state.projectile_id.is_none());
}

#[test]
fn gather_input_sends_at_swing_impact_and_repeats_while_held() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Hatchet;
    let duration = swing_duration_seconds(model);
    let impact_time = duration * swing_impact_fraction(model);

    let tick = state.update(
        0.01,
        true,
        true,
        Some(model),
        Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
            4,
        ))),
        SwingFeelScales::default(),
    );
    assert!(tick.is_none());
    assert!(state.swing_fraction() > 0.0);

    let impact = state
        .update(
            impact_time,
            false,
            true,
            Some(model),
            Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                4,
            ))),
            SwingFeelScales::default(),
        )
        .expect("impact should emit at the impact fraction of the swing");
    assert_eq!(
        impact.target,
        Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
            4
        )))
    );
    assert_eq!(impact.model, model);

    // Same swing, no second impact even though we step further.
    assert!(
        state
            .update(
                0.01,
                false,
                true,
                Some(model),
                Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                    4
                ))),
                SwingFeelScales::default(),
            )
            .is_none()
    );

    let _ = state.update(
        duration,
        false,
        true,
        Some(model),
        Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
            5,
        ))),
        SwingFeelScales::default(),
    );
    // Swing rolled over into a new swing while LMB is held.
    assert!(state.swing_fraction() < 0.2);
}

#[test]
fn gather_input_emits_exactly_one_impact_event_at_impact_fraction() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Pickaxe;
    let duration = swing_duration_seconds(model);
    let impact_time = duration * swing_impact_fraction(model);

    // Up to one frame before the impact threshold: nothing fires.
    let pre_impact = impact_time - 0.001;
    assert!(
        state
            .update(
                pre_impact,
                true,
                true,
                Some(model),
                Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                    7
                ))),
                SwingFeelScales::default(),
            )
            .is_none()
    );

    // Crossing the impact threshold emits exactly one event.
    let impact = state
        .update(
            0.005,
            false,
            true,
            Some(model),
            Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                7,
            ))),
            SwingFeelScales::default(),
        )
        .expect("impact should emit once we cross the impact fraction");
    assert_eq!(
        impact.target,
        Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
            7
        )))
    );
    assert_eq!(impact.model, model);

    // No duplicate impact for the remainder of the swing.
    assert!(
        state
            .update(
                duration * 0.1,
                false,
                false,
                Some(model),
                Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                    7
                ))),
                SwingFeelScales::default(),
            )
            .is_none()
    );
}

#[test]
fn gather_input_swings_without_target_and_yields_no_impact() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Pickaxe;
    let duration = swing_duration_seconds(model);
    let impact_time = duration * swing_impact_fraction(model);

    // Click with no target, swing still starts.
    let _ = state.update(
        0.01,
        true,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    assert!(state.swing_fraction() > 0.0);

    // Crossing the impact fraction emits a SwingImpact with no target.
    let impact = state
        .update(
            impact_time,
            false,
            true,
            Some(model),
            None,
            SwingFeelScales::default(),
        )
        .expect("impact frame should still fire");
    assert!(impact.target.is_none());
    assert_eq!(impact.model, model);
}

#[test]
fn gather_input_does_nothing_without_a_tool_equipped() {
    let mut state = GatherInputState::default();
    assert!(
        state
            .update(
                0.01,
                true,
                true,
                None,
                Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
                    4
                ))),
                SwingFeelScales::default()
            )
            .is_none()
    );
    assert_eq!(state.swing_fraction(), 0.0);
}

#[test]
fn release_before_swing_completes_stops_after_one_impact() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Hatchet;
    let duration = swing_duration_seconds(model);

    // Start a swing with a single click (just_pressed, not held).
    let _ = state.update(
        0.0,
        true,
        false,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    assert!(state.swing_fraction() >= 0.0);

    // Drive the whole swing with no further press. After it completes,
    // it must NOT roll into a new swing (pressed = false).
    let _ = state.update(
        duration,
        false,
        false,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    assert_eq!(
        state.swing_fraction(),
        0.0,
        "a released swing should end rather than repeat"
    );
}

#[test]
fn cancel_clears_active_swing_and_pending_feedback() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Pickaxe;
    let _ = state.update(
        0.01,
        true,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    state.set_pending_impact(PendingImpactEffect {
        anchor: Vec3::ZERO,
        spray_direction: Vec3::Y,
        kind: ImpactEffectKind::StoneShards,
        seed: 1,
    });
    state.set_pending_audio_cue(PendingAudioCue {
        anchor: Vec3::ZERO,
        model,
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
        model: ItemModel::Bag,
        surface: SurfaceMaterial::Dirt,
        is_player_hit: true,
    };
    state.set_pending_audio_cue(cue);
    let taken = state.take_pending_audio_cue().expect("cue present");
    assert!(taken.is_player_hit);
    assert!(state.take_pending_audio_cue().is_none());
}

#[test]
fn hit_stop_holds_the_swing_phase_then_resumes() {
    // Pure phase-math test for the attacker-side hit-stop: after `trigger_hit_stop`
    // the active swing's fraction must NOT advance for the length of the window,
    // then resume advancing once the window elapses.
    let mut state = GatherInputState::default();
    let model = ItemModel::Club;

    // Start a swing and step it a little so it is mid-flight.
    let _ = state.update(
        0.05,
        true,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    let before = state.swing_fraction();
    assert!(before > 0.0, "the swing is under way");

    // Arm a neutral (1.0) hit-stop: the phase must freeze for HIT_STOP_SECONDS.
    state.trigger_hit_stop(1.0);

    // A step SHORTER than the window advances nothing (the phase is held).
    let _ = state.update(
        HIT_STOP_SECONDS * 0.5,
        false,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    assert_eq!(
        state.swing_fraction(),
        before,
        "the swing phase is frozen during the hit-stop window"
    );

    // Burn off the rest of the window (still frozen through the exact window).
    let _ = state.update(
        HIT_STOP_SECONDS * 0.5,
        false,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    assert_eq!(
        state.swing_fraction(),
        before,
        "still frozen at the end of the window (advance resumes the NEXT step)"
    );

    // Now that the window has elapsed, the swing advances again.
    let _ = state.update(
        0.05,
        false,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    assert!(
        state.swing_fraction() > before,
        "the swing phase resumes advancing once the hit-stop elapses"
    );
}

#[test]
fn hit_stop_scale_zero_disables_the_freeze() {
    // A 0 scale (the Dev slider's disable setting) must arm no freeze at all, so
    // the swing advances normally on the very next step.
    let mut state = GatherInputState::default();
    let model = ItemModel::Pickaxe;
    let _ = state.update(
        0.05,
        true,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    let before = state.swing_fraction();
    state.trigger_hit_stop(0.0);
    let _ = state.update(
        0.02,
        false,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(9))),
        SwingFeelScales::default(),
    );
    assert!(
        state.swing_fraction() > before,
        "a 0 hit-stop scale leaves the swing free to advance immediately"
    );
}

#[test]
fn each_started_swing_advances_the_seed() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Bag;
    let duration = swing_duration_seconds(model);
    // Land each swing (real target) so it rolls straight into the next
    // while held; a whiff would charge the miss-recovery gap instead and
    // not roll over until it elapsed.
    let target = Some(SwingTarget::ResourceNode(crate::protocol::ResourceNodeId(
        4,
    )));

    let _ = state.update(
        0.0,
        true,
        true,
        Some(model),
        target,
        SwingFeelScales::default(),
    );
    let first = state.current_swing_seed();
    // Roll into a second swing by completing the first while held.
    let _ = state.update(
        duration,
        false,
        true,
        Some(model),
        target,
        SwingFeelScales::default(),
    );
    let second = state.current_swing_seed();
    assert_ne!(first, second, "each swing should bump the seed");
}

#[test]
fn missed_swing_locks_out_the_next_until_recovery_elapses() {
    use crate::game_balance::COMBAT_MISS_RECOVERY_SECONDS;

    let mut state = GatherInputState::default();
    let model = ItemModel::Hatchet;
    let duration = swing_duration_seconds(model);

    // Hold LMB through a full swing that connects with nothing.
    let _ = state.update(
        0.0,
        true,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    let impact = state
        .update(
            duration,
            false,
            true,
            Some(model),
            None,
            SwingFeelScales::default(),
        )
        .expect("the whiff still emits an impact event");
    assert!(impact.target.is_none());

    // The held button must NOT roll straight into a new swing: the miss
    // recovery gap is in effect, so the tool stays idle.
    let _ = state.update(
        0.0,
        false,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
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
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    assert_eq!(
        state.swing_fraction(),
        0.0,
        "half the recovery window is not enough to resume swinging"
    );

    // Once the rest of the gap elapses, the held button starts a new swing.
    let _ = state.update(
        COMBAT_MISS_RECOVERY_SECONDS,
        false,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    assert!(
        state.swing_fraction() > 0.0,
        "the swing resumes once the recovery gap has elapsed"
    );
}

#[test]
fn landed_swing_repeats_with_no_recovery_penalty() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Hatchet;
    let duration = swing_duration_seconds(model);
    let target = Some(SwingTarget::Player(crate::protocol::ClientId(7)));

    // A swing that lands on a player should roll straight into the next
    // one while LMB is held, no recovery gap, full cadence preserved.
    let _ = state.update(
        0.0,
        true,
        true,
        Some(model),
        target,
        SwingFeelScales::default(),
    );
    let _ = state.update(
        duration,
        false,
        true,
        Some(model),
        target,
        SwingFeelScales::default(),
    );
    assert!(
        state.swing_fraction() < 0.2,
        "a landed swing repeats immediately while held"
    );
}

#[test]
fn tool_swap_or_death_clears_a_pending_miss_recovery() {
    let mut state = GatherInputState::default();
    let model = ItemModel::Hatchet;
    let duration = swing_duration_seconds(model);

    // Whiff to arm the recovery gap.
    let _ = state.update(
        0.0,
        true,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );
    let _ = state.update(
        duration,
        false,
        true,
        Some(model),
        None,
        SwingFeelScales::default(),
    );

    // Cancel (tool swap / death) should wipe the lockout so the next tool
    // is usable immediately rather than inheriting a stun.
    state.cancel();
    let _ = state.update(
        0.01,
        true,
        true,
        Some(model),
        Some(SwingTarget::Player(crate::protocol::ClientId(3))),
        SwingFeelScales::default(),
    );
    assert!(
        state.swing_fraction() > 0.0,
        "after cancel a fresh click swings without waiting out the old recovery"
    );
}

#[test]
fn impact_effect_kind_maps_models_and_surfaces() {
    use crate::resource_nodes::ResourceNodeModel;
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

/// Every [`ItemModel`] swing archetype. The exhaustive match forces a new
/// variant to be added here, so the completeness test below then covers it and a
/// model can never reach the swing machine without a duration + contact fraction.
const ALL_SWING_MODELS: [ItemModel; 11] = [
    ItemModel::Bag,
    ItemModel::Deployable,
    ItemModel::Hatchet,
    ItemModel::Pickaxe,
    ItemModel::Club,
    ItemModel::Spear,
    ItemModel::Sword,
    ItemModel::Bow,
    ItemModel::Crossbow,
    ItemModel::ThrownBomb,
    ItemModel::Sickle,
];

#[test]
fn all_swing_models_listed_and_have_valid_timing() {
    // The exhaustive match makes adding an ItemModel a compile error until it is
    // slotted into ALL_SWING_MODELS, so the completeness assertions below (and
    // the neutral-timing test) actually cover every archetype.
    fn listed(model: ItemModel) -> bool {
        match model {
            ItemModel::Bag
            | ItemModel::Deployable
            | ItemModel::Hatchet
            | ItemModel::Pickaxe
            | ItemModel::Club
            | ItemModel::Spear
            | ItemModel::Sword
            | ItemModel::Bow
            | ItemModel::Crossbow
            | ItemModel::ThrownBomb
            | ItemModel::Sickle
            | ItemModel::Bandage => true,
        }
    }
    assert!(ALL_SWING_MODELS.iter().copied().all(listed));

    // Every archetype must resolve to a positive, finite duration and a contact
    // fraction strictly inside the swing (never on the first or last frame). The
    // pose-dispatch half of the completeness check (every model animates) lives
    // in held.rs's own test module, next to the dispatch it exercises.
    for model in ALL_SWING_MODELS {
        let duration = swing_duration_seconds(model);
        assert!(
            duration.is_finite() && duration > 0.0,
            "{model:?} needs a positive duration"
        );
        let fraction = swing_impact_fraction(model);
        assert!(
            fraction > 0.0 && fraction < 1.0,
            "{model:?} contact fraction {fraction} must sit inside the swing"
        );
    }
}

#[test]
fn weapon_swing_durations_preserve_the_speed_ordering() {
    // Club fastest, then sword, then spear, mirroring the server cooldown
    // ordering (club < sword < spear). The swing the player feels must
    // keep that order so the weapons stay distinguishable.
    let club = swing_duration_seconds(ItemModel::Club);
    let sword = swing_duration_seconds(ItemModel::Sword);
    let spear = swing_duration_seconds(ItemModel::Spear);
    assert!(club < sword, "club swings faster than the sword");
    assert!(sword < spear, "sword swings faster than the spear");
}

#[test]
fn swing_feel_neutral_reproduces_shipped_timing() {
    // Neutral scales must reproduce the exact shipped duration and impact
    // fraction for every archetype, so a release build and an untouched dev
    // session swing byte-for-byte identically.
    let feel = SwingFeelScales::default();
    for model in ALL_SWING_MODELS {
        assert_eq!(feel.duration_for(model), swing_duration_seconds(model));
        assert_eq!(
            feel.impact_fraction_for(model),
            swing_impact_fraction(model)
        );
    }
}

#[test]
fn swing_feel_duration_scale_stretches_and_compresses() {
    let model = ItemModel::Hatchet;
    let base = swing_duration_seconds(model);
    let slow = SwingFeelScales {
        duration_scale: 2.0,
        impact_fraction_offset: 0.0,
    };
    let fast = SwingFeelScales {
        duration_scale: 0.5,
        impact_fraction_offset: 0.0,
    };
    assert!((slow.duration_for(model) - base * 2.0).abs() < 1e-6);
    assert!((fast.duration_for(model) - base * 0.5).abs() < 1e-6);
}

#[test]
fn swing_feel_duration_scale_guards_against_zero_and_nan() {
    // A pathological scale (0 or non-finite) must fall back to neutral rather
    // than freeze the swing with a zero-length duration.
    let model = ItemModel::Pickaxe;
    let base = swing_duration_seconds(model);
    let zero = SwingFeelScales {
        duration_scale: 0.0,
        impact_fraction_offset: 0.0,
    };
    let nan = SwingFeelScales {
        duration_scale: f32::NAN,
        impact_fraction_offset: 0.0,
    };
    assert_eq!(zero.duration_for(model), base);
    assert_eq!(nan.duration_for(model), base);
}

#[test]
fn swing_feel_impact_offset_shifts_and_clamps() {
    use crate::app::state::{DEV_COMBAT_IMPACT_FRACTION_MAX, DEV_COMBAT_IMPACT_FRACTION_MIN};
    let model = ItemModel::Hatchet;
    let base = swing_impact_fraction(model);

    // A small offset shifts the fraction by exactly that amount.
    let shifted = SwingFeelScales {
        duration_scale: 1.0,
        impact_fraction_offset: 0.1,
    };
    assert!((shifted.impact_fraction_for(model) - (base + 0.1)).abs() < 1e-6);

    // A huge positive offset clamps to the upper bound, not past 1.0.
    let over = SwingFeelScales {
        duration_scale: 1.0,
        impact_fraction_offset: 5.0,
    };
    assert_eq!(
        over.impact_fraction_for(model),
        DEV_COMBAT_IMPACT_FRACTION_MAX
    );

    // A huge negative offset clamps to the lower bound, not below 0.0.
    let under = SwingFeelScales {
        duration_scale: 1.0,
        impact_fraction_offset: -5.0,
    };
    assert_eq!(
        under.impact_fraction_for(model),
        DEV_COMBAT_IMPACT_FRACTION_MIN
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
fn ranged_timing_entries_are_fire_recovery_beats_not_placeholders() {
    // P3b retires the P3a bag placeholders: the ranged archetypes carry their own
    // fire-recovery beats. The crossbow's recovery is longer than the bow's (the
    // heavier bolt's recoil), both contact fractions sit early in the beat (the
    // shot leaves at the start; the rest is settle), and neither rides the bag
    // punch cadence any more. The draw HOLD itself lives in RangedDrawState, not
    // this table; these beats are the post-fire settle only.
    let bow = swing_duration_seconds(ItemModel::Bow);
    let crossbow = swing_duration_seconds(ItemModel::Crossbow);
    let bag = swing_duration_seconds(ItemModel::Bag);
    assert_ne!(bow, bag, "the bow no longer rides the bag placeholder");
    assert_ne!(
        crossbow, bag,
        "the crossbow no longer rides the bag placeholder"
    );
    assert!(crossbow > bow, "the heavier crossbow recovers slower");

    assert!(
        swing_impact_fraction(ItemModel::Bow) < 0.5,
        "the bow's shot leaves early in the fire-recovery beat"
    );
    assert!(
        swing_impact_fraction(ItemModel::Crossbow) < 0.5,
        "the crossbow's shot leaves early in the fire-recovery beat"
    );

    // Swap weights: the bow lifts like the light hatchet; the crossbow shoulders
    // slowly like the pickaxe (heavy iron machinery).
    assert_eq!(
        swap_duration_for_model(ItemModel::Bow),
        swap_duration_for_model(ItemModel::Hatchet)
    );
    assert_eq!(
        swap_duration_for_model(ItemModel::Crossbow),
        swap_duration_for_model(ItemModel::Pickaxe)
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
