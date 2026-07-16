//! The audio manifest: one enum, one defaults table, one paths function.
//!
//! Adding a sound is now:
//! 1. Drop the WAV/OGG file(s) under `assets/<subdir>/`.
//! 2. Add a `SoundId` variant.
//! 3. Add one row to [`sound_defaults`] for the mix defaults.
//! 4. Add one row to [`sound_paths`] for the asset path(s).
//!
//! Variant pools are declared as a static slice of `&'static str` paths.
//! For sequentially-numbered pools (e.g. `footstep-dirt-01.wav` … `-12.wav`)
//! [`footstep_paths`] builds the path list once at startup so adding more
//! variants is "drop the new files, change the count".

use std::sync::OnceLock;

use crate::items::ItemModel;

use super::{category::SoundCategory, surface::SurfaceMaterial};

/// Every sound the client can play. Compile-time exhaustive so missing a
/// case in [`sound_defaults`] / [`sound_paths`] is a build error, not a
/// runtime silent-failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SoundId {
    // --- UI ---
    UiButtonClick,
    UiButtonHover,

    // --- Music ---
    MainMenuMusic,

    // --- Transitions (one-shot stingers for game-state changes) ---
    WorldJoin,

    // --- World one-shots ---
    TreeFall,

    // --- Tool impacts: (tool, surface successfully struck) ---
    ImpactAxeOnWood,
    /// Axe striking anything that isn't wood (stone vein, ore, stone
    /// structures). Mixed down from the pickaxe-ore pool, same hard-
    /// surface transient, pitched up so it reads as the lighter hatchet
    /// rather than the heavier pickaxe.
    ImpactAxeGeneric,
    ImpactPickaxeOnStone,
    ImpactPickaxeOnCoal,
    ImpactPickaxeOnIron,
    ImpactPickaxeOnSulfur,
    /// Pickaxe striking a wood entity (tree, wood structure). Mixed
    /// down from the axe-wood pool, same wood-fracture transient,
    /// pitched down so it reads as the heavier pickaxe rather than the
    /// lighter hatchet.
    ImpactPickaxeOnWood,

    /// An ore/vein node crossing a visual depletion-stage threshold (the
    /// mound visibly breaking down a size). Derived offline from the
    /// pickaxe-ore pool: two takes layered 45 ms apart, pitched down 3
    /// and 6 semitones, lowpassed at 3.2 kHz with a fast fade-out from
    /// 0.26 s, a short, tight slump of rock under the strike rather than
    /// another pick transient.
    OreStageCrumble,
    /// The "node finished" reward when an ore/vein is mined empty, the
    /// signal to stop swinging (trees get the same from [`Self::TreeFall`]).
    /// The satisfaction comes from weight, not brightness: anything
    /// tonal up top (tuned chimes, sparkle) read as birdsong / MMO
    /// level-up in playtests, so the mix is deliberately all low-mid.
    /// Derived offline: a -2-semitone ore crack over a -12-semitone
    /// lowpassed boom (the chest-thump), a short lowpassed slice of the
    /// tree-fall crash climax for full-spectrum body, and a quiet
    /// -8-semitone settle at 90 ms; everything is gone by half a second.
    OreNodeBreak,

    // --- Swing whoosh (tool swung but no target) ---
    SwingMiss,

    /// PvP melee impact ("thump" of a blunt tool landing on a player).
    /// The gather-tool (hatchet/pickaxe) PvP pool. Routed off the existing
    /// axe-wood pool until dedicated foley lands, see `impact_sound_for_player`.
    ImpactPlayerBlunt,
    /// PvP impact for the wooden club: the blunter of the two impact character
    /// sets (aliases the pickaxe-ore hard-thud pool). ALIAS, swap `sound_paths`
    /// to real club-on-flesh foley when recorded.
    ImpactPlayerClub,
    /// PvP impact for the stone spear: a sharper puncture read (aliases the
    /// axe-wood "hatchet-vs-flesh" pool, the closest existing thump). ALIAS.
    ImpactPlayerSpear,
    /// PvP impact for the iron sword: the sharper of the two sets (aliases the
    /// axe-wood pool, matching the hatchet's brighter transient). ALIAS.
    ImpactPlayerSword,
    /// PvP impact for the iron mace: the heaviest blunt read (aliases the
    /// pickaxe-ore hard-thud pool, the bluntest existing set). ALIAS.
    ImpactPlayerMace,

    // --- Ranged (bow / crossbow) cues ---
    //
    // Deliberately SPARSE: every draw / release / whoosh / fire / reload cue
    // tried so far read wrong in play (owner reports), so the shot itself is
    // silent; only the dry-click and the spatial arrow-lodge impact remain.
    /// Ranged dry-click: pulling the trigger on a crossbow still on reload
    /// cooldown (or a bow with no arrow). A quiet, no-shot cue. Aliases the
    /// door-code-wrong dull knock. Non-spatial.
    RangedDryClick,
    /// An arrow/bolt lodging in the world (tree, stone, ground, wall): a sharp
    /// snap into a wood-body knock with a faint shaft-vibration buzz. Backed by
    /// the synthesized `impacts/arrow-impact-*.wav` pool; the old routing fell
    /// into the tool-impact pools and read as a pickaxe striking rock (owner
    /// report). Spatial at the impact point.
    ImpactArrowWorld,

    // --- Footsteps per surface ---
    FootstepDirt,
    FootstepWood,
    FootstepConcrete,
    FootstepSand,

    // --- Inventory cues ---
    InventoryPickup,
    /// Material-matched pickup variant for wood (branch piles, dropped
    /// wood stacks): the grass-brush rustle with three light twig ticks
    /// scattered through it, sticks snatched up off the ground. Derived
    /// offline: `pickup-item.wav` (+9 dB) under the first 80-90 ms of
    /// `axe-wood-1/3/2` attack transients (pitched up 9/6/11 semitones,
    /// highpassed, fast-faded so only the snap remains) at 40/130/210 ms.
    /// An earlier wood-footstep version read as hollow door knocks.
    PickupSticks,
    /// Material-matched pickup variant for stone and ore chunks: two dry
    /// pebble clacks with a gravelly settle. Derived offline from the
    /// concrete footstep pool (`concrete-03/08/05` pitched up 5/8/2
    /// semitones at 0/70/130 ms, highpassed at 250-300 Hz).
    PickupStones,
    InventoryDrop,
    InventoryMove,

    // --- Progress cues (something the player queued has finished) ---
    /// A crafting job completed and its output landed in the bag.
    /// Derived offline from `inventory/pickup-item.wav` (pitched down
    /// 2 semitones) so it reads as a weightier "finished work" landing.
    CraftComplete,
    /// A furnace went cold (smelt batch finished or fuel ran out), worth
    /// walking over to check. Derived offline from
    /// `impacts/pickaxe-ore-1.wav` (pitched down 5 semitones, lowpass,
    /// short stone tail) so it reads as the last bar settling in the
    /// firebox.
    FurnaceComplete,
    /// Actionbar slot selection (number key or wheel). Derived offline
    /// from `ui/button-click.wav` (pitched up 3 semitones), a lighter,
    /// shorter tick than the menu click.
    HotbarSelect,

    // --- Doors ---
    /// Keypad accepted the entered code: the dry latch click with a
    /// brighter +4-semitone tick on top at 90 ms. Derived offline from
    /// `ui/button-click.wav`.
    DoorCodeCorrect,
    /// Keypad rejected the code: two dull low knocks. Derived offline
    /// from `ui/button-click.wav` pitched down ~5 and ~6.5 semitones,
    /// lowpassed at 1.0-1.2 kHz, second knock at 140 ms.
    DoorCodeWrong,
    /// A door panel starting to swing: a soft latch release
    /// (`ui/button-click.wav` down 2 semitones) over a low wood shift
    /// (`footsteps/wood-04.wav` down 6 semitones, slowed 15%,
    /// lowpassed at 2 kHz).
    DoorOpen,
    /// A door panel falling shut: a heavy wood thunk
    /// (`footsteps/wood-07.wav` down 8 semitones, lowpassed) ended by
    /// the latch catching (`ui/button-click.wav` down 1 semitone at
    /// 110 ms).
    DoorClose,

    // --- meteor shower ---
    /// Retired approach-roar cue. The dedicated `world/meteor-flyby.wav` crossing
    /// bed now carries the whole approach-and-pass shape, so there is no separate
    /// roar to trigger (folding it in avoids a doubled sound). Kept as a variant
    /// (still pathed + defaulted so the manifest stays exhaustive) but no longer
    /// written by the renderer. Aliases the flyby bed.
    MeteorShowerRoar,
    /// The impact explosion boom, played spatially at the crater with a distance
    /// delay (light-then-sound). Backed by `world/meteor-impact.wav`, a 6.0 s mono
    /// one-shot: leading silence trimmed, and the source's flat 4.5 s drag tail
    /// (lumpy re-swell artifacts; owner report) faded out from 4.6 s so the boom
    /// decays clean instead of rumbling on.
    MeteorShowerImpact,
    /// The distance-muffled variant of the impact boom: the same file through a
    /// double 450 Hz low-pass, RMS-matched to the source
    /// (`ffmpeg -af "lowpass=f=450,lowpass=f=450,volume=1dB"`). Played
    /// NON-spatially for strikes heard from far away (or from behind shelter),
    /// where air absorption strips the crack and leaves the low thump; the
    /// renderer picks it over the crisp file past the muffle handoff distance.
    MeteorShowerImpactFar,
    /// The meteor's crossing/approach rumble: `world/meteor-flyby.wav`, an 18 s
    /// stereo bed with its own approach-then-pass shape. Played ONCE per event
    /// (NOT looped, a loop would restart mid-descent and sound wrong), non-spatial,
    /// with the renderer scaling its gain each frame by proximity (inaudible when
    /// the object is kilometres out, swelling as it screams overhead). Timed to end
    /// at impact so the `MeteorShowerImpact` boom takes over cleanly.
    MeteorShowerRumble,

    // --- Explosives ---
    /// A placed charge's fuse hiss, played spatially at the charge and re-fired
    /// on a short cadence while the camera is near so it reads as a continuous
    /// sizzle a defender can locate by ear. Dedicated foley
    /// (`explosions/fuse-sizzle-*.wav`): two windows cut straight from the
    /// reference fuse recording's steady burn, RMS-matched so the refire
    /// cadence overlaps into one continuous sizzle.
    FuseHiss,
    /// The detonation: a low physical thump at the blast, played spatially so a
    /// nearby player feels the weight of the breach. Dedicated foley
    /// (`explosions/explosion-close-*.wav`): instant attack, short body, tail
    /// cut hard (fully decayed by ~1.3-1.7 s) so nothing echoes or lingers.
    ExplosionThump,
    /// The far rumble that trails the thump, played spatially at the blast with a
    /// distance delay (`distance / speed_of_sound`) so a distant breach lands as
    /// flash-then-rumble. Dedicated foley (`explosions/explosion-far-*.wav`),
    /// low-passed for the distance read, trimmed so the swell arrives promptly
    /// (the delay is the consumer's job, not dead air in the file).
    ExplosionRumble,
    /// The thrower's own release cue when a powder bomb leaves the hand at the
    /// toss pose's release frame: a short front-loaded air-cut whoosh
    /// (`explosions/throw-whoosh-1.wav`). Non-spatial (the local thrower's own
    /// toss).
    BombThrowRelease,
    /// An admin `/tp` yanked this player across the map: a swell-then-pass
    /// whoosh (`transitions/teleport-whoosh.wav`) so the relocation reads as an
    /// event instead of a silent camera jump. Non-spatial (it belongs to the
    /// teleported player, not a point in the world).
    TeleportWhoosh,
}

/// Returns every defined sound. Useful for the loader at startup so we
/// can warm decoder handles for the full set in one pass.
pub(crate) fn all_sound_ids() -> &'static [SoundId] {
    &[
        SoundId::UiButtonClick,
        SoundId::UiButtonHover,
        SoundId::MainMenuMusic,
        SoundId::WorldJoin,
        SoundId::TreeFall,
        SoundId::ImpactAxeOnWood,
        SoundId::ImpactAxeGeneric,
        SoundId::ImpactPickaxeOnStone,
        SoundId::ImpactPickaxeOnCoal,
        SoundId::ImpactPickaxeOnIron,
        SoundId::ImpactPickaxeOnSulfur,
        SoundId::ImpactPickaxeOnWood,
        SoundId::OreStageCrumble,
        SoundId::OreNodeBreak,
        SoundId::ImpactPlayerBlunt,
        SoundId::ImpactPlayerClub,
        SoundId::ImpactPlayerSpear,
        SoundId::ImpactPlayerSword,
        SoundId::ImpactPlayerMace,
        SoundId::RangedDryClick,
        SoundId::ImpactArrowWorld,
        SoundId::SwingMiss,
        SoundId::FootstepDirt,
        SoundId::FootstepWood,
        SoundId::FootstepConcrete,
        SoundId::FootstepSand,
        SoundId::InventoryPickup,
        SoundId::PickupSticks,
        SoundId::PickupStones,
        SoundId::InventoryDrop,
        SoundId::InventoryMove,
        SoundId::CraftComplete,
        SoundId::FurnaceComplete,
        SoundId::HotbarSelect,
        SoundId::DoorCodeCorrect,
        SoundId::DoorCodeWrong,
        SoundId::DoorOpen,
        SoundId::DoorClose,
        SoundId::MeteorShowerRoar,
        SoundId::MeteorShowerImpact,
        SoundId::MeteorShowerImpactFar,
        SoundId::MeteorShowerRumble,
        SoundId::FuseHiss,
        SoundId::ExplosionThump,
        SoundId::ExplosionRumble,
        SoundId::BombThrowRelease,
        SoundId::TeleportWhoosh,
    ]
}

/// Mix-bus defaults for a sound. Carried by the pool so the per-fire
/// `PlaySound` call only needs to supply optional overrides.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SoundDefaults {
    pub(crate) category: SoundCategory,
    /// Reference gain in dB before slider scaling. Lands the *peak* of the
    /// recording at this level when the user's slider sits at 1.0.
    pub(crate) base_gain_db: f32,
    /// Spatial parameters; `None` for non-spatial one-shots and 2D loops.
    pub(crate) spatial: Option<SpatialDefaults>,
    /// Per-fire random pitch range, applied as a multiplicative speed
    /// factor: `speed = 1.0 + uniform(-pitch_jitter, +pitch_jitter)`. `0.0`
    /// disables. Heavy one-shots (tree-fall, music) should stay at `0.0`
    /// so they don't sound off-pitch on replay.
    pub(crate) pitch_jitter: f32,
    /// Whether the sound loops. Looped sounds skip the polyphony cap and
    /// produce a long-lived entity the caller can despawn.
    pub(crate) looped: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SpatialDefaults {
    /// Rodio's spatial scale (gain = (1 / (scale·d)²).min(1.0)). Lower
    /// values extend the full-volume zone. 0.06 ≈ ~16 m of full gain.
    pub(crate) scale: f32,
    /// Vertical offset above the supplied anchor, in metres. Lifts the
    /// source closer to ear height so it doesn't sound like it's coming
    /// from the floor.
    pub(crate) height_offset: f32,
}

/// Match each variant to its mix defaults. Compile-time exhaustive, add
/// a `SoundId` variant and the compiler points at the missing arm here.
pub(crate) const fn sound_defaults(id: SoundId) -> SoundDefaults {
    match id {
        // Chrome cues sit well below the mix; the click is more present
        // than the hover so menu interactions feel weighty.
        SoundId::UiButtonClick => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -12.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        SoundId::UiButtonHover => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -30.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },

        // Menu music sits at -24 dB to leave headroom for hover/click cues
        // playing over it.
        SoundId::MainMenuMusic => SoundDefaults {
            category: SoundCategory::Music,
            base_gain_db: -24.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: true,
        },

        // Transition stinger when the player enters a world (singleplayer
        // or multiplayer). Mixed at the same loudness reference as the
        // menu music so it doesn't blow the level when the music is
        // still fading out beneath it. Music-category routing keeps it
        // off the SFX slider, players adjust this with the same control
        // they use for the soundtrack, since it's a "scoring" cue, not a
        // gameplay event. Non-spatial, no pitch jitter (a signature
        // sound should always play the same way), uncapped polyphony
        // (only one entry transition happens at a time anyway).
        //
        // -9 dB lands the stinger at ~70% of the original -6 dB level
        // (20·log10(0.7) ≈ -3.1 dB). Earlier mixes at -6 dB rode a bit
        // hot over the fading menu music; the trim leaves headroom for
        // the in-game ambience swelling under it.
        SoundId::WorldJoin => SoundDefaults {
            category: SoundCategory::Music,
            base_gain_db: -9.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },

        // Tree-fall is the most significant world event in the second it
        // plays, but loud enough at 0 dBFS in the source to overpower the
        // mix; -12 dB lands the crash near impact-cue loudness. The crash
        // anchors at the trunk base; lifting the source 1.5 m puts it
        // closer to ear height.
        SoundId::TreeFall => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -12.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.5,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },

        // Per-hit impact cues, short, sharp transients. Pitch jitter ±5%
        // gives every swing audible variety without a third pre-rendered
        // variant per pool.
        SoundId::ImpactAxeOnWood
        | SoundId::ImpactAxeGeneric
        | SoundId::ImpactPickaxeOnStone
        | SoundId::ImpactPickaxeOnCoal
        | SoundId::ImpactPickaxeOnIron
        | SoundId::ImpactPickaxeOnSulfur
        | SoundId::ImpactPickaxeOnWood => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -10.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.0,
            }),
            pitch_jitter: 0.05,
            looped: false,
        },

        // Stage-crossing crumble: plays layered under the same swing's
        // pick transient, so it sits a touch above the impact pool and
        // relies on its lower register (not level) to read as a separate
        // event. The crack belongs to the rock near the ground, hence
        // the lower height offset than the strike cues.
        SoundId::OreStageCrumble => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -9.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 0.6,
            }),
            pitch_jitter: 0.05,
            looped: false,
        },

        // Node-finished break: the completion signature, slightly louder
        // than the per-hit pool so it lands as the event that ends the
        // mining loop, and jitter-free so it always sounds the same (a
        // signal the player learns, like the tree-fall crash).
        SoundId::OreNodeBreak => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -8.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 0.6,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },

        // PvP melee impact, a meatier thump than chipping at a tree, so it sits a
        // bit louder than the resource impact pool. Wider pitch jitter (±9 %)
        // because rapid hits would otherwise sound metronomic; the body of a
        // player gives a different resonance each time. Every weapon's PvP pool
        // shares these mix defaults; only the sample set (see `sound_paths`)
        // differs, so recording real per-weapon foley is a one-line swap.
        SoundId::ImpactPlayerBlunt
        | SoundId::ImpactPlayerClub
        | SoundId::ImpactPlayerSpear
        | SoundId::ImpactPlayerSword
        | SoundId::ImpactPlayerMace => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -8.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.0,
            }),
            pitch_jitter: 0.09,
            looped: false,
        },

        // Dry-click: a quiet negative-feedback knock, deliberately understated so
        // it reads as "not yet" without nagging.
        SoundId::RangedDryClick => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -18.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        // Arrow lodging in the world: spatial at the impact point, mixed like
        // the tool-impact pool with the same variety jitter.
        SoundId::ImpactArrowWorld => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -10.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.0,
            }),
            pitch_jitter: 0.05,
            looped: false,
        },

        // Miss whoosh belongs to the local swinger, non-spatial so
        // distance falloff can't quiet the player's own swing.
        SoundId::SwingMiss => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -10.0,
            spatial: None,
            pitch_jitter: 0.05,
            looped: false,
        },

        // Footsteps cover their per-material loudness offset via the
        // base_gain_db here. Each material's pool was captured at a
        // different level; baking it into the manifest replaces a parallel
        // gain-offset switch. Pitch jitter ±3% gives subtle variation that
        // reads as natural footfall rather than identical repeats.
        SoundId::FootstepDirt => SoundDefaults {
            category: SoundCategory::Footsteps,
            base_gain_db: -8.0 + 13.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepWood => SoundDefaults {
            category: SoundCategory::Footsteps,
            base_gain_db: -8.0 + -7.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepConcrete => SoundDefaults {
            category: SoundCategory::Footsteps,
            base_gain_db: -8.0 + 3.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepSand => SoundDefaults {
            category: SoundCategory::Footsteps,
            base_gain_db: -8.0 + 12.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },

        // Inventory pickup, a trimmed real grass-rustle recording, so it
        // reads as brushing the item out of the grass rather than a chime
        // or a clicky metal clink. The source clip is ~21 dB quieter than
        // the old cue, so base_gain_db is raised far above the drop/move
        // cues just to reach a comparable, deliberately subtle in-game
        // level (it sits a few dB under where the old pickup landed). ±4 %
        // pitch jitter keeps a rapid-fire pickup burst from sounding like
        // a metronome.
        SoundId::InventoryPickup => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: 0.0,
            spatial: None,
            pitch_jitter: 0.04,
            looped: false,
        },
        // Material pickup variants. Their files peak near full scale
        // (unlike the very quiet grass-rustle recording above), so they
        // take a real trim to land at the same deliberately subtle level
        // the pickup family sits at. ±5% jitter keeps a gathering spree
        // from machine-gunning one identical rattle.
        SoundId::PickupSticks | SoundId::PickupStones => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -19.0,
            spatial: None,
            pitch_jitter: 0.05,
            looped: false,
        },
        // Drop cue, slightly more body, hits a touch quieter than pickup
        // because dropping is a deliberate negative-feedback action, not
        // an achievement.
        SoundId::InventoryDrop => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -18.0,
            spatial: None,
            pitch_jitter: 0.04,
            looped: false,
        },
        // Slot-shuffle tick, UI chrome, deliberately quiet so dragging a
        // stack across the grid doesn't drown out gameplay audio.
        SoundId::InventoryMove => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -28.0,
            spatial: None,
            pitch_jitter: 0.05,
            looped: false,
        },
        // Craft-complete lands between the pickup cue and the UI chrome:
        // audible over ambience as a small reward, no jitter so the
        // completion signature always sounds the same.
        SoundId::CraftComplete => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -14.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        // Furnace shutoff anchors at the furnace so the player can locate
        // which one went quiet by ear; carries like the impact pool.
        SoundId::FurnaceComplete => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -8.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.0,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },
        // Hotbar slot tick, quieter than the menu click; slight jitter so
        // scrolling across slots doesn't machine-gun one identical tick.
        SoundId::HotbarSelect => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -22.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        // Keypad feedback for the player at the door: quiet chrome, no
        // jitter (a lock always sounds like itself).
        SoundId::DoorCodeCorrect | SoundId::DoorCodeWrong => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -16.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        // Door swings are world events everyone nearby hears; light
        // jitter so a base full of doors doesn't sound copy-pasted.
        SoundId::DoorOpen | SoundId::DoorClose => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -15.0,
            spatial: Some(SpatialDefaults {
                scale: 0.06,
                height_offset: 1.2,
            }),
            pitch_jitter: 0.04,
            looped: false,
        },

        // Retired: the flyby bed now carries the approach, so the roar is never
        // triggered. Kept defaulted (non-spatial, non-looped) so the manifest
        // stays exhaustive; the level is moot since it is not written anymore.
        SoundId::MeteorShowerRoar => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -4.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        // The impact boom is spatial at the crater and a loud one-shot (NOT
        // looped). A wide full-gain zone (small scale) so the 6.0 s explosion
        // still reads as a heavy blast from a fair way off, matching the map-wide
        // plume.
        SoundId::MeteorShowerImpact => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -2.0,
            spatial: Some(SpatialDefaults {
                scale: 0.012,
                height_offset: 2.0,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },
        // The muffled far boom: same reference level as the crisp file (the
        // renderer's per-instance distance gain does ALL the level work, so
        // the two variants stay directly comparable), non-spatial because a
        // kilometres-out thump has no meaningful bearing through Bevy's
        // spatial panner and its rolloff would double-count the distance.
        SoundId::MeteorShowerImpactFar => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -2.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
        // The crossing rumble is a non-spatial ONE-SHOT bed (NOT looped: the file
        // has its own approach-then-pass shape, and a loop would restart it
        // mid-descent). The renderer scales its gain by proximity each frame (the
        // announce gives it the meteor's world position, so a manifest spatial
        // falloff would double-count), starting it near-silent and swelling it
        // toward this reference as the fireball closes, so a distant meteor is a
        // whisper and an overhead pass a felt roar. No pitch jitter (a signature
        // cue).
        SoundId::MeteorShowerRumble => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -3.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },

        // Fuse hiss: quiet + spatial at the charge so it does not dominate but is
        // locatable by ear when close. A tight spatial scale (fast falloff) so a
        // charge across a base does not hiss in your ear. Light jitter so the
        // re-fired sample does not machine-gun one identical hiss. Dropped from
        // -20 after the switch to the reference-sample sizzle (owner feedback:
        // the burn should sit under the action, never dominate it).
        SoundId::FuseHiss => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -29.0,
            spatial: Some(SpatialDefaults {
                scale: 0.10,
                height_offset: 0.4,
            }),
            pitch_jitter: 0.06,
            looped: false,
        },
        // Detonation thump: loud, spatial, no jitter (a signature event should
        // sound the same each time). A wide full-gain zone (small scale) so a
        // breach reads heavy from a fair way off, matching the far rumble.
        SoundId::ExplosionThump => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -3.0,
            spatial: Some(SpatialDefaults {
                scale: 0.02,
                height_offset: 1.0,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },
        // Far rumble: the delayed tail. Slightly quieter than the thump and a
        // wider full-gain zone so it carries; the distance delay (scheduled by the
        // consumer) is what sells the flash-then-sound.
        SoundId::ExplosionRumble => SoundDefaults {
            category: SoundCategory::Sfx3d,
            base_gain_db: -6.0,
            spatial: Some(SpatialDefaults {
                scale: 0.012,
                height_offset: 1.5,
            }),
            pitch_jitter: 0.0,
            looped: false,
        },
        // Bomb release: the local thrower's own toss cue, non-spatial so it always
        // reads regardless of aim, a touch quiet (it punctuates the toss, not an
        // alarm). Light jitter so a rapid toss/toss does not machine-gun one whoosh.
        // The dedicated whoosh peaks ~9 dB hotter than the old miss-pool alias
        // (-3 dBFS vs -12), so the base gain drops by the same amount to keep the
        // perceived level.
        SoundId::BombThrowRelease => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -21.0,
            spatial: None,
            pitch_jitter: 0.05,
            looped: false,
        },
        // Teleport: the moved player's own relocation cue, non-spatial (you are
        // the destination). No jitter: a signature event should sound like
        // itself. Mixed clearly audible but not startling; an admin /tp already
        // surprises the player enough.
        SoundId::TeleportWhoosh => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -10.0,
            spatial: None,
            pitch_jitter: 0.0,
            looped: false,
        },
    }
}

/// Returns the asset paths for a sound's variant pool. Each path is
/// relative to `assets/`. The same path appearing twice means deliberate
/// duplication, but in practice every entry is a separate recording or a
/// pre-rendered variant.
pub(crate) fn sound_paths(id: SoundId) -> &'static [&'static str] {
    static UI_CLICK: [&str; 1] = ["ui/button-click.wav"];
    static UI_HOVER: [&str; 1] = ["ui/button-hover.wav"];
    static MENU_MUSIC: [&str; 1] = ["music/main-menu.wav"];
    static WORLD_JOIN: [&str; 1] = ["transitions/world-join.wav"];
    static TREE_FALL: [&str; 1] = ["world/tree-fall.wav"];

    static AXE_WOOD: [&str; 3] = [
        "impacts/axe-wood-1.wav",
        "impacts/axe-wood-2.wav",
        "impacts/axe-wood-3.wav",
    ];
    // Hatchet hitting anything but wood. Derived offline from
    // pickaxe-ore-*.wav (pitched up +2 semitones, gain -1 dB) so the
    // strike reads as the lighter hatchet against a hard surface.
    static AXE_GENERIC: [&str; 3] = [
        "impacts/axe-generic-1.wav",
        "impacts/axe-generic-2.wav",
        "impacts/axe-generic-3.wav",
    ];
    static PICKAXE_ORE: [&str; 3] = [
        "impacts/pickaxe-ore-1.wav",
        "impacts/pickaxe-ore-2.wav",
        "impacts/pickaxe-ore-3.wav",
    ];
    // Pickaxe biting into a wood entity. Derived offline from
    // axe-wood-*.wav (pitched down ~3 semitones, gain +1.2 dB) so the
    // strike reads as the heavier pickaxe rather than the hatchet.
    static PICKAXE_WOOD: [&str; 3] = [
        "impacts/pickaxe-wood-1.wav",
        "impacts/pickaxe-wood-2.wav",
        "impacts/pickaxe-wood-3.wav",
    ];
    static MISS: [&str; 3] = [
        "impacts/miss-1.wav",
        "impacts/miss-2.wav",
        "impacts/miss-3.wav",
    ];
    // Depletion-stage crumble and node-finished break, see the
    // `OreStageCrumble` / `OreNodeBreak` variant docs for the offline
    // derivation recipes.
    static ORE_CRUMBLE: [&str; 1] = ["impacts/ore-crumble.wav"];
    static ORE_BREAK: [&str; 1] = ["impacts/ore-break.wav"];

    // PvP player-impact pools. Every weapon aliases an existing impact sample
    // set chosen for its character until dedicated foley is recorded, at which
    // point each becomes a one-line `static` swap:
    //   - The "softer / sharper" set (axe-wood) reads as a hatchet-bright
    //     transient: the gather-tool PvP thump, the sword, and the spear
    //     (hatchet-vs-flesh) route here.
    //   - The "harder / blunter" set (pickaxe-ore) reads as a heavy hard thud:
    //     the club and the mace route here.
    static PLAYER_BLUNT_SOFT: [&str; 3] = [
        "impacts/axe-wood-1.wav",
        "impacts/axe-wood-2.wav",
        "impacts/axe-wood-3.wav",
    ];
    static PLAYER_BLUNT_HARD: [&str; 3] = [
        "impacts/pickaxe-ore-1.wav",
        "impacts/pickaxe-ore-2.wav",
        "impacts/pickaxe-ore-3.wav",
    ];

    // Ranged cue pools. Deliberately sparse: the shot itself is silent (every
    // launch cue tried read wrong in play, owner reports), so only the
    // dry-click (aliasing the door-code-wrong dull knock) and the arrow
    // impact pool remain.
    static RANGED_DRY_CLICK: [&str; 1] = ["ui/door-code-wrong.wav"];
    static ARROW_IMPACT: [&str; 3] = [
        "impacts/arrow-impact-1.wav",
        "impacts/arrow-impact-2.wav",
        "impacts/arrow-impact-3.wav",
    ];

    static INVENTORY_PICKUP: [&str; 1] = ["inventory/pickup-item.wav"];
    static PICKUP_STICKS: [&str; 1] = ["inventory/pickup-sticks.wav"];
    static PICKUP_STONES: [&str; 1] = ["inventory/pickup-stones.wav"];
    static INVENTORY_DROP: [&str; 1] = ["inventory/drop-item.wav"];
    static INVENTORY_MOVE: [&str; 1] = ["inventory/inventory-move.wav"];
    static CRAFT_COMPLETE: [&str; 1] = ["crafting/craft-complete.wav"];
    static FURNACE_COMPLETE: [&str; 1] = ["crafting/furnace-complete.wav"];
    static HOTBAR_SELECT: [&str; 1] = ["ui/hotbar-select.wav"];
    static DOOR_CODE_CORRECT: [&str; 1] = ["ui/door-code-correct.wav"];
    static DOOR_CODE_WRONG: [&str; 1] = ["ui/door-code-wrong.wav"];
    static DOOR_OPEN: [&str; 1] = ["world/door-open.wav"];
    static DOOR_CLOSE: [&str; 1] = ["world/door-close.wav"];

    // Dedicated meteor foley. The crossing/approach bed (and the retired roar,
    // folded into it) ride the flyby; the impact rides the explosion boom.
    static METEOR_FLYBY: [&str; 1] = ["world/meteor-flyby.wav"];
    static METEOR_IMPACT: [&str; 1] = ["world/meteor-impact.wav"];
    static METEOR_IMPACT_FAR: [&str; 1] = ["world/meteor-impact-far.wav"];

    // Dedicated explosion foley (trimmed / mono-folded / normalized offline).
    // close-1 and close-2 are cut straight from the user-supplied reference
    // explosion samples; close-3 and both far rumbles are Mixkit recordings
    // (free license, no attribution) tightened to the same short no-echo
    // profile; the sizzles are two windows of the reference fuse recording's
    // steady burn, re-fired on the fuse cadence.
    static EXPLOSION_CLOSE: [&str; 3] = [
        "explosions/explosion-close-1.wav",
        "explosions/explosion-close-2.wav",
        "explosions/explosion-close-3.wav",
    ];
    static EXPLOSION_FAR: [&str; 2] = [
        "explosions/explosion-far-1.wav",
        "explosions/explosion-far-2.wav",
    ];
    static FUSE_SIZZLE: [&str; 2] = [
        "explosions/fuse-sizzle-1.wav",
        "explosions/fuse-sizzle-2.wav",
    ];
    static THROW_WHOOSH: [&str; 1] = ["explosions/throw-whoosh-1.wav"];

    // Cut from the user-supplied swoosh reference set: a swell-then-pass
    // whoosh for the admin-/tp relocation cue.
    static TELEPORT_WHOOSH: [&str; 1] = ["transitions/teleport-whoosh.wav"];

    match id {
        SoundId::UiButtonClick => &UI_CLICK,
        SoundId::UiButtonHover => &UI_HOVER,
        SoundId::MainMenuMusic => &MENU_MUSIC,
        SoundId::WorldJoin => &WORLD_JOIN,
        SoundId::TreeFall => &TREE_FALL,
        SoundId::ImpactAxeOnWood => &AXE_WOOD,
        SoundId::ImpactAxeGeneric => &AXE_GENERIC,
        // Until each ore has its own captured impact pool, every pickaxe
        // ore-hit shares the existing ore-node recording. New pools land
        // by adding files under `assets/items/` and pointing this match
        // arm at them.
        SoundId::ImpactPickaxeOnStone
        | SoundId::ImpactPickaxeOnCoal
        | SoundId::ImpactPickaxeOnIron
        | SoundId::ImpactPickaxeOnSulfur => &PICKAXE_ORE,
        SoundId::ImpactPickaxeOnWood => &PICKAXE_WOOD,
        SoundId::OreStageCrumble => &ORE_CRUMBLE,
        SoundId::OreNodeBreak => &ORE_BREAK,
        // Gather-tool PvP thump, the sword, and the spear share the sharper
        // (axe-wood) set; the club and mace share the blunter (pickaxe-ore) set.
        SoundId::ImpactPlayerBlunt | SoundId::ImpactPlayerSword | SoundId::ImpactPlayerSpear => {
            &PLAYER_BLUNT_SOFT
        }
        SoundId::ImpactPlayerClub | SoundId::ImpactPlayerMace => &PLAYER_BLUNT_HARD,
        SoundId::RangedDryClick => &RANGED_DRY_CLICK,
        SoundId::ImpactArrowWorld => &ARROW_IMPACT,
        SoundId::SwingMiss => &MISS,
        SoundId::FootstepDirt => footstep_paths(FootstepMaterial::Dirt),
        SoundId::FootstepWood => footstep_paths(FootstepMaterial::Wood),
        SoundId::FootstepConcrete => footstep_paths(FootstepMaterial::Concrete),
        SoundId::FootstepSand => footstep_paths(FootstepMaterial::Sand),
        SoundId::InventoryPickup => &INVENTORY_PICKUP,
        SoundId::PickupSticks => &PICKUP_STICKS,
        SoundId::PickupStones => &PICKUP_STONES,
        SoundId::InventoryDrop => &INVENTORY_DROP,
        SoundId::InventoryMove => &INVENTORY_MOVE,
        SoundId::CraftComplete => &CRAFT_COMPLETE,
        SoundId::FurnaceComplete => &FURNACE_COMPLETE,
        SoundId::HotbarSelect => &HOTBAR_SELECT,
        SoundId::DoorCodeCorrect => &DOOR_CODE_CORRECT,
        SoundId::DoorCodeWrong => &DOOR_CODE_WRONG,
        SoundId::DoorOpen => &DOOR_OPEN,
        SoundId::DoorClose => &DOOR_CLOSE,
        // Dedicated meteor foley. The retired roar and the crossing rumble both
        // ride the flyby bed (the roar is no longer triggered); the impact rides
        // the explosion boom.
        SoundId::MeteorShowerRoar | SoundId::MeteorShowerRumble => &METEOR_FLYBY,
        SoundId::MeteorShowerImpact => &METEOR_IMPACT,
        SoundId::MeteorShowerImpactFar => &METEOR_IMPACT_FAR,
        // Dedicated explosion foley (see the statics above). The old aliases
        // (miss whoosh for the fuse, tree-fall for the boom) are gone.
        SoundId::FuseHiss => &FUSE_SIZZLE,
        SoundId::ExplosionThump => &EXPLOSION_CLOSE,
        SoundId::ExplosionRumble => &EXPLOSION_FAR,
        SoundId::BombThrowRelease => &THROW_WHOOSH,
        SoundId::TeleportWhoosh => &TELEPORT_WHOOSH,
    }
}

/// Map a swing archetype ([`ItemModel`]) + struck surface to the impact
/// `SoundId` to play for a resource / deployable hit. Returns `None` for pairs
/// that have no dedicated sound; callers should fall back to the swing whoosh.
///
/// Keyed on the wire impact identity: gather hits arrive as the Hatchet /
/// Pickaxe archetype (the hammer's repair tap resolves to Hatchet too, so it is
/// covered by the hatchet arm). Weapon archetypes never reach here (a weapon hit
/// is a player hit, routed through [`impact_sound_for_player`], or a
/// hands-tier deployable hit that maps to the hatchet cue).
pub(crate) fn impact_sound_for(model: ItemModel, surface: SurfaceMaterial) -> Option<SoundId> {
    // Collapse the swing archetype into its resource-impact family. The
    // bag / deployable (bare-hand) archetype has no dedicated impact clip, it
    // reports `None` exactly as the old `ToolKind::Hands` arm did, so a crude
    // hand-pick (branch pile, surface stone, hay) stays silent on the remote path
    // and falls back to the whoosh locally. A gather hit arrives as Hatchet or
    // Pickaxe; a weapon that struck a deployable resolves to its own archetype
    // and reads as the pickaxe (heavy) or hatchet (everything else) family.
    let heavy_pick = match model {
        ItemModel::Pickaxe | ItemModel::Mace => true,
        ItemModel::Hatchet | ItemModel::Club | ItemModel::Spear | ItemModel::Sword => false,
        // The sickle's only gatherable surface is a grass tuft, so its
        // contact cue IS the fiber-collection sound: the same grass-rustle
        // the hand E-pluck plays (owner request), not a tool clang.
        ItemModel::Sickle => return Some(SoundId::InventoryPickup),
        // An arrow/bolt lodging in the world gets its own cue on every surface;
        // the tool pools read as a pickaxe chipping rock (owner report).
        ItemModel::Bow | ItemModel::Crossbow => return Some(SoundId::ImpactArrowWorld),
        // No dedicated impact clip for the empty-hand / deployable punch, the
        // thrown bomb (its damage is the blast, whose cue is the explosion audio,
        // not a swing-contact clip), or the bandage (it never strikes anything).
        ItemModel::Bag | ItemModel::Deployable | ItemModel::ThrownBomb | ItemModel::Bandage => {
            return None;
        }
    };
    match (heavy_pick, surface) {
        (false, SurfaceMaterial::Wood) => Some(SoundId::ImpactAxeOnWood),
        // Hatchet-class biting anything else (ore, stone vein, stone structure,
        // hay, dirt). Generic mixed-down pickaxe-ore sample.
        (false, _) => Some(SoundId::ImpactAxeGeneric),
        (true, SurfaceMaterial::Wood) => Some(SoundId::ImpactPickaxeOnWood),
        (true, SurfaceMaterial::Stone) => Some(SoundId::ImpactPickaxeOnStone),
        (true, SurfaceMaterial::Coal) => Some(SoundId::ImpactPickaxeOnCoal),
        (true, SurfaceMaterial::Iron) => Some(SoundId::ImpactPickaxeOnIron),
        (true, SurfaceMaterial::Sulfur) => Some(SoundId::ImpactPickaxeOnSulfur),
        (true, SurfaceMaterial::Dirt | SurfaceMaterial::Concrete | SurfaceMaterial::Sand) => None,
    }
}

/// PvP-impact sound lookup, keyed on the swing archetype. Each melee weapon
/// routes to its own PvP pool (aliasing an existing sample set until dedicated
/// foley lands); the gather tools share the generic blunt pool. The bag /
/// deployable archetype produces no PvP sound (bare hands can't damage players;
/// the server rejects it).
pub(crate) fn impact_sound_for_player(model: ItemModel) -> Option<SoundId> {
    match model {
        // Gather tools (a desperation weapon) share the generic blunt thump.
        ItemModel::Hatchet | ItemModel::Pickaxe | ItemModel::Sickle => {
            Some(SoundId::ImpactPlayerBlunt)
        }
        ItemModel::Club => Some(SoundId::ImpactPlayerClub),
        ItemModel::Spear => Some(SoundId::ImpactPlayerSpear),
        ItemModel::Sword => Some(SoundId::ImpactPlayerSword),
        ItemModel::Mace => Some(SoundId::ImpactPlayerMace),
        // Ranged hits share the generic blunt thump as a placeholder; P3b's feel
        // pass gives arrow impacts their own cue.
        ItemModel::Bow | ItemModel::Crossbow => Some(SoundId::ImpactPlayerBlunt),
        // Bare hands / deployable-in-hand can't damage players, a thrown bomb
        // does no contact damage (its blast is the damage), and a bandage does no
        // damage at all, so none produce a PvP-contact cue.
        ItemModel::Bag | ItemModel::Deployable | ItemModel::ThrownBomb | ItemModel::Bandage => None,
    }
}

/// The surface materials that have a dedicated footstep recording pool. Using
/// an enum here (rather than a `&str`) keeps [`footstep_paths`] total: the match
/// is exhaustive at compile time, so there is no "unknown material" panic arm a
/// future typo could trip.
#[derive(Clone, Copy)]
enum FootstepMaterial {
    Dirt,
    Wood,
    Concrete,
    Sand,
}

impl FootstepMaterial {
    /// Filename prefix under `assets/footsteps/`.
    const fn prefix(self) -> &'static str {
        match self {
            FootstepMaterial::Dirt => "dirt",
            FootstepMaterial::Wood => "wood",
            FootstepMaterial::Concrete => "concrete",
            FootstepMaterial::Sand => "sand",
        }
    }
}

/// Lazily-built `Vec<String>` of `footsteps/<material>-01.wav` …
/// `-12.wav`. Cached behind a `OnceLock` per material so the pool array
/// is built exactly once per process. The pool size matches the embedded
/// asset count, drop more files in and bump `12`.
fn footstep_paths(material: FootstepMaterial) -> &'static [&'static str] {
    fn pool_for(prefix: &'static str) -> &'static [&'static str] {
        // Twelve variants per material; the anti-repeat picker can't
        // produce an audible loop at running cadence with a pool this big.
        let strings: Vec<&'static str> = (1..=12)
            .map(|n| {
                let owned = format!("footsteps/{prefix}-{n:02}.wav");
                // Leak: cheap, one-time per material at startup. Returning
                // `&'static str` keeps the call site allocation-free.
                Box::leak(owned.into_boxed_str()) as &'static str
            })
            .collect();
        Box::leak(strings.into_boxed_slice())
    }

    static DIRT: OnceLock<&'static [&'static str]> = OnceLock::new();
    static WOOD: OnceLock<&'static [&'static str]> = OnceLock::new();
    static CONCRETE: OnceLock<&'static [&'static str]> = OnceLock::new();
    static SAND: OnceLock<&'static [&'static str]> = OnceLock::new();

    let slot = match material {
        FootstepMaterial::Dirt => &DIRT,
        FootstepMaterial::Wood => &WOOD,
        FootstepMaterial::Concrete => &CONCRETE,
        FootstepMaterial::Sand => &SAND,
    };
    slot.get_or_init(|| pool_for(material.prefix()))
}

/// Map a [`SurfaceMaterial`] to the footstep `SoundId` that plays when
/// standing on it. Surfaces without a dedicated pool fall back to dirt.
pub(crate) fn footstep_sound_for(surface: SurfaceMaterial) -> SoundId {
    match surface {
        SurfaceMaterial::Dirt => SoundId::FootstepDirt,
        SurfaceMaterial::Wood => SoundId::FootstepWood,
        SurfaceMaterial::Concrete => SoundId::FootstepConcrete,
        SurfaceMaterial::Sand => SoundId::FootstepSand,
        // Ores and stone fall back to dirt until they get their own pool.
        SurfaceMaterial::Stone
        | SurfaceMaterial::Iron
        | SurfaceMaterial::Coal
        | SurfaceMaterial::Sulfur => SoundId::FootstepDirt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_id_has_at_least_one_path() {
        for id in all_sound_ids() {
            let paths = sound_paths(*id);
            assert!(!paths.is_empty(), "{id:?} has no paths declared");
        }
    }

    #[test]
    fn looped_sounds_skip_pitch_jitter() {
        // Music and ambient loops must never randomly pitch-shift, it
        // would sound wrong on every cycle. Enforce that the manifest
        // never accidentally configures them with jitter.
        for id in all_sound_ids() {
            let defaults = sound_defaults(*id);
            if defaults.looped {
                assert_eq!(
                    defaults.pitch_jitter, 0.0,
                    "{id:?} is looped but has pitch_jitter, would warble"
                );
            }
        }
    }

    #[test]
    fn footstep_paths_contain_twelve_numbered_variants() {
        let dirt = sound_paths(SoundId::FootstepDirt);
        assert_eq!(dirt.len(), 12);
        assert_eq!(dirt[0], "footsteps/dirt-01.wav");
        assert_eq!(dirt[11], "footsteps/dirt-12.wav");
    }

    #[test]
    fn impact_table_covers_canonical_pairs() {
        assert_eq!(
            impact_sound_for(ItemModel::Hatchet, SurfaceMaterial::Wood),
            Some(SoundId::ImpactAxeOnWood)
        );
        assert_eq!(
            impact_sound_for(ItemModel::Pickaxe, SurfaceMaterial::Iron),
            Some(SoundId::ImpactPickaxeOnIron)
        );
        // Hatchet on a non-wood surface (e.g. striking a furnace) ships the
        // mixed-down generic axe impact.
        assert_eq!(
            impact_sound_for(ItemModel::Hatchet, SurfaceMaterial::Iron),
            Some(SoundId::ImpactAxeGeneric)
        );
        assert_eq!(
            impact_sound_for(ItemModel::Hatchet, SurfaceMaterial::Stone),
            Some(SoundId::ImpactAxeGeneric)
        );
        // Pickaxe on wood (e.g. striking a workbench) ships the mixed-down
        // pickaxe-on-wood impact.
        assert_eq!(
            impact_sound_for(ItemModel::Pickaxe, SurfaceMaterial::Wood),
            Some(SoundId::ImpactPickaxeOnWood)
        );
        // The bag (empty-hand / deployable-in-hand) archetype reports `None` so
        // the fallback stays explicit.
        assert_eq!(
            impact_sound_for(ItemModel::Bag, SurfaceMaterial::Wood),
            None
        );
    }

    #[test]
    fn every_item_model_resolves_a_pvp_or_no_sound() {
        // Completeness: `impact_sound_for_player` is total over the whole
        // `ItemModel` enum. Every weapon archetype resolves to a distinct pool;
        // gather tools share the blunt pool; the non-combat archetypes report
        // `None` (they cannot damage players).
        assert_eq!(
            impact_sound_for_player(ItemModel::Club),
            Some(SoundId::ImpactPlayerClub)
        );
        assert_eq!(
            impact_sound_for_player(ItemModel::Spear),
            Some(SoundId::ImpactPlayerSpear)
        );
        assert_eq!(
            impact_sound_for_player(ItemModel::Sword),
            Some(SoundId::ImpactPlayerSword)
        );
        assert_eq!(
            impact_sound_for_player(ItemModel::Mace),
            Some(SoundId::ImpactPlayerMace)
        );
        assert_eq!(
            impact_sound_for_player(ItemModel::Hatchet),
            Some(SoundId::ImpactPlayerBlunt)
        );
        assert_eq!(
            impact_sound_for_player(ItemModel::Pickaxe),
            Some(SoundId::ImpactPlayerBlunt)
        );
        assert_eq!(impact_sound_for_player(ItemModel::Bag), None);
        assert_eq!(impact_sound_for_player(ItemModel::Deployable), None);

        // And every archetype resolves a resource impact sound or a defensible
        // `None` (never a panic): iterating `ItemModel::ALL` proves totality.
        for &model in ItemModel::ALL {
            let _ = impact_sound_for(model, SurfaceMaterial::Wood);
            let _ = impact_sound_for_player(model);
        }
    }

    #[test]
    fn ranged_cues_alias_the_expected_existing_pools() {
        // The P3b ranged cues are aliases onto existing sample sets until
        // dedicated foley lands. Pin each alias so a swap of one cue's pool can't
        // silently drift the others, and so recording real foley is a visible,
        // one-line `sound_paths` change with a test that flips with it. This is
        // the ranged analogue of the PvP alias coverage above.
        assert_eq!(
            sound_paths(SoundId::RangedDryClick),
            &["ui/door-code-wrong.wav"]
        );
        // Arrows lodging in the world carry their own synthesized pool on every
        // surface (the tool pools read as a pickaxe chipping rock, owner report).
        assert_eq!(
            sound_paths(SoundId::ImpactArrowWorld),
            &[
                "impacts/arrow-impact-1.wav",
                "impacts/arrow-impact-2.wav",
                "impacts/arrow-impact-3.wav",
            ]
        );
        for surface in [
            SurfaceMaterial::Wood,
            SurfaceMaterial::Stone,
            SurfaceMaterial::Dirt,
        ] {
            assert_eq!(
                impact_sound_for(ItemModel::Bow, surface),
                Some(SoundId::ImpactArrowWorld)
            );
            assert_eq!(
                impact_sound_for(ItemModel::Crossbow, surface),
                Some(SoundId::ImpactArrowWorld)
            );
        }
    }

    #[test]
    fn explosive_cues_bind_the_dedicated_explosion_foley() {
        // The fuse, both blast cues, and the throw release bind the dedicated
        // recordings under `assets/explosions/` (the old miss-whoosh /
        // tree-fall aliases are retired). Pin the paths so a manifest
        // regression is loud.
        assert_eq!(
            sound_paths(SoundId::FuseHiss),
            &[
                "explosions/fuse-sizzle-1.wav",
                "explosions/fuse-sizzle-2.wav",
            ]
        );
        assert_eq!(
            sound_paths(SoundId::ExplosionThump),
            &[
                "explosions/explosion-close-1.wav",
                "explosions/explosion-close-2.wav",
                "explosions/explosion-close-3.wav",
            ]
        );
        assert_eq!(
            sound_paths(SoundId::ExplosionRumble),
            &[
                "explosions/explosion-far-1.wav",
                "explosions/explosion-far-2.wav",
            ]
        );
        assert_eq!(
            sound_paths(SoundId::BombThrowRelease),
            &["explosions/throw-whoosh-1.wav"]
        );
    }

    #[test]
    fn every_manifest_path_points_at_a_real_asset_file() {
        // The library loads paths lazily, so a typo'd path only surfaces as a
        // silent missing sound at runtime. Pin every pool entry to a file in
        // the repo's `assets/` so CI catches a rename or a forgotten commit.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        for id in all_sound_ids() {
            for path in sound_paths(*id) {
                assert!(
                    root.join(path).is_file(),
                    "{id:?} points at missing asset {path}"
                );
            }
        }
    }

    #[test]
    fn teleport_whoosh_is_a_non_spatial_local_cue() {
        // The teleport whoosh belongs to the moved player (you ARE the
        // destination), so it must stay non-spatial: a spatial mix would race
        // the ear position against the snap itself.
        assert_eq!(
            sound_paths(SoundId::TeleportWhoosh),
            &["transitions/teleport-whoosh.wav"]
        );
        assert!(sound_defaults(SoundId::TeleportWhoosh).spatial.is_none());
    }

    #[test]
    fn explosive_cues_are_spatial_so_distance_reads() {
        // The fuse hiss and both blast cues are world events located in space, so
        // each must declare spatial defaults (a defender locates a charge by ear,
        // and a distant breach reads by its falloff). This is the explosive
        // completeness check: every explosive cue resolves both a path (above) and
        // a spatial mix here.
        for id in [
            SoundId::FuseHiss,
            SoundId::ExplosionThump,
            SoundId::ExplosionRumble,
        ] {
            assert!(
                sound_defaults(id).spatial.is_some(),
                "{id:?} must be a spatial world cue"
            );
        }
    }

    #[test]
    fn ranged_cues_are_non_spatial_local_player_cues() {
        // The one remaining local ranged cue (the dry-click) belongs to the
        // shooter, so it must be non-spatial: distance falloff must never
        // quiet a player's own weapon. The arrow-impact cue is spatial by
        // design (it belongs to the lodge point, not the shooter).
        assert!(
            sound_defaults(SoundId::RangedDryClick).spatial.is_none(),
            "the dry-click should be a non-spatial local-player cue"
        );
    }
}
