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
//! [`numbered_pool`] generates the path list at compile time so adding more
//! variants is "drop the new files, change the count".

use std::sync::OnceLock;

use crate::items::ToolKind;

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
    /// structures). Mixed down from the pickaxe-ore pool — same hard-
    /// surface transient, pitched up so it reads as the lighter hatchet
    /// rather than the heavier pickaxe.
    ImpactAxeGeneric,
    ImpactPickaxeOnStone,
    ImpactPickaxeOnCoal,
    ImpactPickaxeOnIron,
    ImpactPickaxeOnSulfur,
    /// Pickaxe striking a wood entity (tree, wood structure). Mixed
    /// down from the axe-wood pool — same wood-fracture transient,
    /// pitched down so it reads as the heavier pickaxe rather than the
    /// lighter hatchet.
    ImpactPickaxeOnWood,

    // --- Swing whoosh (tool swung but no target) ---
    SwingMiss,

    // --- Footsteps per surface ---
    FootstepDirt,
    FootstepWood,
    FootstepConcrete,
    FootstepSand,

    // --- Inventory cues ---
    InventoryPickup,
    InventoryDrop,
    InventoryMove,
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
        SoundId::SwingMiss,
        SoundId::FootstepDirt,
        SoundId::FootstepWood,
        SoundId::FootstepConcrete,
        SoundId::FootstepSand,
        SoundId::InventoryPickup,
        SoundId::InventoryDrop,
        SoundId::InventoryMove,
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

/// Match each variant to its mix defaults. Compile-time exhaustive — add
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
        // off the SFX slider — players adjust this with the same control
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

        // Per-hit impact cues — short, sharp transients. Pitch jitter ±5%
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

        // Miss whoosh belongs to the local swinger — non-spatial so
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
            category: SoundCategory::Sfx2d,
            base_gain_db: -8.0 + 13.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepWood => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -8.0 + -7.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepConcrete => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -8.0 + 3.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },
        SoundId::FootstepSand => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -8.0 + 12.0,
            spatial: None,
            pitch_jitter: 0.03,
            looped: false,
        },

        // Inventory pickup — sharp, bright, but kept well below impact
        // cues so it reads as confirmation, not an achievement chime.
        // ±4 % pitch jitter keeps a rapid-fire pickup burst from sounding
        // like a metronome.
        SoundId::InventoryPickup => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -16.0,
            spatial: None,
            pitch_jitter: 0.04,
            looped: false,
        },
        // Drop cue — slightly more body, hits a touch quieter than pickup
        // because dropping is a deliberate negative-feedback action, not
        // an achievement.
        SoundId::InventoryDrop => SoundDefaults {
            category: SoundCategory::Sfx2d,
            base_gain_db: -18.0,
            spatial: None,
            pitch_jitter: 0.04,
            looped: false,
        },
        // Slot-shuffle tick — UI chrome, deliberately quiet so dragging a
        // stack across the grid doesn't drown out gameplay audio.
        SoundId::InventoryMove => SoundDefaults {
            category: SoundCategory::Ui,
            base_gain_db: -28.0,
            spatial: None,
            pitch_jitter: 0.05,
            looped: false,
        },
    }
}

/// Returns the asset paths for a sound's variant pool. Each path is
/// relative to `assets/`. The same path appearing twice means deliberate
/// duplication — but in practice every entry is a separate recording or a
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

    static INVENTORY_PICKUP: [&str; 1] = ["inventory/pickup-item.wav"];
    static INVENTORY_DROP: [&str; 1] = ["inventory/drop-item.wav"];
    static INVENTORY_MOVE: [&str; 1] = ["inventory/inventory-move.wav"];

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
        SoundId::SwingMiss => &MISS,
        SoundId::FootstepDirt => footstep_paths("dirt"),
        SoundId::FootstepWood => footstep_paths("wood"),
        SoundId::FootstepConcrete => footstep_paths("concrete"),
        SoundId::FootstepSand => footstep_paths("sand"),
        SoundId::InventoryPickup => &INVENTORY_PICKUP,
        SoundId::InventoryDrop => &INVENTORY_DROP,
        SoundId::InventoryMove => &INVENTORY_MOVE,
    }
}

/// Map a (tool, surface) pair to the impact `SoundId` to play. Returns
/// `None` for pairs that have no dedicated sound — callers should fall
/// back to the swing whoosh in that case.
///
/// New combinations slot in by adding a row here. The audio-selection
/// table replaces the old `ImpactEffectKind`-keyed dispatch, which was
/// stuck at "tree → wood chips, anything else → stone shards".
pub(crate) fn impact_sound_for(tool: ToolKind, surface: SurfaceMaterial) -> Option<SoundId> {
    match (tool, surface) {
        (ToolKind::Axe, SurfaceMaterial::Wood) => Some(SoundId::ImpactAxeOnWood),
        // Hatchet biting anything else (ore, stone vein, stone structure,
        // hay, dirt). Generic mixed-down pickaxe-ore sample.
        (ToolKind::Axe, _) => Some(SoundId::ImpactAxeGeneric),
        (ToolKind::Pickaxe, SurfaceMaterial::Wood) => Some(SoundId::ImpactPickaxeOnWood),
        (ToolKind::Pickaxe, SurfaceMaterial::Stone) => Some(SoundId::ImpactPickaxeOnStone),
        (ToolKind::Pickaxe, SurfaceMaterial::Coal) => Some(SoundId::ImpactPickaxeOnCoal),
        (ToolKind::Pickaxe, SurfaceMaterial::Iron) => Some(SoundId::ImpactPickaxeOnIron),
        (ToolKind::Pickaxe, SurfaceMaterial::Sulfur) => Some(SoundId::ImpactPickaxeOnSulfur),
        // Bare hands never reach here — the input layer suppresses the
        // swing entirely when no real tool is equipped. The arm exists
        // so the match stays exhaustive against future ToolKind /
        // SurfaceMaterial additions.
        (ToolKind::Hands, _)
        | (ToolKind::Pickaxe, SurfaceMaterial::Dirt)
        | (ToolKind::Pickaxe, SurfaceMaterial::Concrete)
        | (ToolKind::Pickaxe, SurfaceMaterial::Sand) => None,
    }
}

/// Lazily-built `Vec<String>` of `movement/footstep-<material>-01.wav` …
/// `-12.wav`. Cached behind a `OnceLock` per material so the pool array
/// is built exactly once per process. The pool size matches the embedded
/// asset count — drop more files in and bump `12`.
fn footstep_paths(material: &'static str) -> &'static [&'static str] {
    fn pool_for(material: &'static str) -> &'static [&'static str] {
        // Twelve variants per material; the anti-repeat picker can't
        // produce an audible loop at running cadence with a pool this big.
        let strings: Vec<&'static str> = (1..=12)
            .map(|n| {
                let owned = format!("footsteps/{material}-{n:02}.wav");
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
        "dirt" => &DIRT,
        "wood" => &WOOD,
        "concrete" => &CONCRETE,
        "sand" => &SAND,
        other => panic!("footstep_paths called with unknown material {other:?}"),
    };
    slot.get_or_init(|| pool_for(material))
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
        // Music and ambient loops must never randomly pitch-shift — it
        // would sound wrong on every cycle. Enforce that the manifest
        // never accidentally configures them with jitter.
        for id in all_sound_ids() {
            let defaults = sound_defaults(*id);
            if defaults.looped {
                assert_eq!(
                    defaults.pitch_jitter, 0.0,
                    "{id:?} is looped but has pitch_jitter — would warble"
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
            impact_sound_for(ToolKind::Axe, SurfaceMaterial::Wood),
            Some(SoundId::ImpactAxeOnWood)
        );
        assert_eq!(
            impact_sound_for(ToolKind::Pickaxe, SurfaceMaterial::Iron),
            Some(SoundId::ImpactPickaxeOnIron)
        );
        // Hatchet on a non-wood surface (e.g. striking a furnace) used
        // to fall through to the miss whoosh — now it ships the
        // mixed-down generic axe impact.
        assert_eq!(
            impact_sound_for(ToolKind::Axe, SurfaceMaterial::Iron),
            Some(SoundId::ImpactAxeGeneric)
        );
        assert_eq!(
            impact_sound_for(ToolKind::Axe, SurfaceMaterial::Stone),
            Some(SoundId::ImpactAxeGeneric)
        );
        // Pickaxe on wood (e.g. striking a workbench) used to fall
        // through to the miss whoosh — now it ships the mixed-down
        // pickaxe-on-wood impact.
        assert_eq!(
            impact_sound_for(ToolKind::Pickaxe, SurfaceMaterial::Wood),
            Some(SoundId::ImpactPickaxeOnWood)
        );
        // Bare hands never reach the dispatcher today, but the table
        // still reports `None` so the fallback stays explicit.
        assert_eq!(
            impact_sound_for(ToolKind::Hands, SurfaceMaterial::Wood),
            None
        );
    }
}
