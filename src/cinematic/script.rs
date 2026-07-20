//! The cinematic shot script: ordered shots, per-shot time-of-day, and the
//! shared phase timings (countdown, intermission, init settle).
//!
//! The server ticks this timeline (`crate::server::cinematic`) and broadcasts
//! a cue at every phase edge; the client looks shots up by index to drive the
//! camera path and the countdown overlay. Camera keys live here next to the
//! shot they frame; all positions are stage world-space (see
//! `super::layout`).

use bevy::math::Vec3;

use super::camera::{CameraKey, CameraPath, key};

/// Seconds of on-screen countdown before every shot. Long enough to reach
/// OBS or settle in, and it gives a clean cut point on either side.
pub const COUNTDOWN_SECONDS: f32 = 5.0;

/// Idle gap between shots (camera holds the last frame of the previous
/// shot). Sized so post can cut generously on both ends of every shot.
pub const INTERMISSION_SECONDS: f32 = 12.0;

/// Settle time after the init phase finishes spawning the stage, before the
/// first countdown starts: lets replication, asset streaming, and grass
/// settle so the first shot opens clean.
pub const INIT_SECONDS: f32 = 4.0;

/// The shot whose start triggers the meteor event ("Starfall").
pub const METEOR_SHOT_INDEX: usize = 4;

/// The PvP shot ("Skirmish"): the fighters escalate while it plays, and the
/// scripted kill lands [`SKIRMISH_KILL_SECONDS`] in.
pub const SKIRMISH_SHOT_INDEX: usize = 3;
pub const SKIRMISH_KILL_SECONDS: f32 = 10.0;

/// The gathering shot ("Harvest"): at its countdown the server pre-drains the
/// hero pine and the iron deposit so both visibly break mid-shot under the
/// actors' real gather swings.
pub const HARVEST_SHOT_INDEX: usize = 1;
/// Real gather swings left on the hero pine when the harvest shot starts.
pub const HARVEST_TREE_SWINGS_LEFT: u32 = 6;
/// Real gather swings left on the iron deposit when the harvest shot starts.
pub const HARVEST_ORE_SWINGS_LEFT: u32 = 8;

/// The building shot ("Homestead"): the builder erects the extension live,
/// stepping through `crate::cinematic::layout::homestead_build_sequence`.
pub const HOMESTEAD_SHOT_INDEX: usize = 2;

/// Pinned trajectory seed for the starfall strike so every take gets the
/// identical entry azimuth and streak (the routine event path derives its
/// seed from the impact tick, which varies per run).
pub const METEOR_TRAJECTORY_SEED: u64 = 0x57A2_FA11_0000_C14E;

/// Warning lead passed to the meteor event when the starfall shot begins.
/// The fireball becomes visible `METEOR_FLIGHT_SECONDS` (10 s) before
/// impact, so with a 12 s warning the streak enters frame 2 s into the shot
/// and impacts at the 12 s mark of the 20 s shot.
pub const METEOR_WARNING_SECONDS: f32 = 12.0;

/// One cinematic shot.
#[derive(Debug, Clone, Copy)]
pub struct Shot {
    /// Display name, shown on the countdown slate.
    pub name: &'static str,
    /// Hour of the world day (0..24) locked in at countdown start so the
    /// shot's lighting is fixed and repeatable.
    pub world_time_hours: f32,
    pub camera: CameraPath,
    /// When set, the camera's look-at target follows the live meteor
    /// fireball while one is in visible flight (falling back to the keyed
    /// look before entry and after impact), so the descent is always
    /// centred regardless of the seeded entry azimuth.
    pub track_meteor: bool,
}

impl Shot {
    pub fn duration_seconds(&self) -> f32 {
        self.camera.duration_seconds()
    }
}

/// Shot 0, "Daybreak": high aerial push over the arena clearing, descending
/// toward the base compound in dawn light. Stays above the treeline until it
/// crosses into the base clear zone.
const DAYBREAK_KEYS: &[CameraKey] = &[
    key(0.0, Vec3::new(-30.0, 16.0, 44.0), Vec3::new(5.0, 0.0, 10.0)),
    key(
        10.0,
        Vec3::new(-2.0, 13.0, 16.0),
        Vec3::new(18.0, 1.0, -12.0),
    ),
    key(
        16.0,
        Vec3::new(13.0, 6.0, -2.0),
        Vec3::new(22.0, 1.8, -14.0),
    ),
];

/// Shot 1, "Harvest": low orbit around the woodcutter at the grove hero
/// pine, ending framed on the mining corner beyond.
const HARVEST_KEYS: &[CameraKey] = &[
    key(0.0, Vec3::new(-31.5, 2.4, 5.5), Vec3::new(-26.0, 3.2, 12.0)),
    key(7.0, Vec3::new(-19.5, 2.0, 7.0), Vec3::new(-26.0, 2.6, 12.0)),
    // Ends a little wide of the arc so the sapling at (-20.5, 11.5) stays
    // out of the lens (verified against a headless capture).
    key(
        14.0,
        Vec3::new(-16.8, 2.2, 10.2),
        Vec3::new(-20.5, 1.5, 17.0),
    ),
];

/// Shot 2, "Homestead": close on the builder hammering the sticks
/// extension, tracking out to a wide reveal of the whole compound. The arc
/// stays WEST of the large pine at (33, -8): the original mid key at
/// (32, -9.5) passed 1.8 m from its trunk and flew straight through the
/// canopy (owner report; verified against headless captures).
const HOMESTEAD_KEYS: &[CameraKey] = &[
    key(
        0.0,
        Vec3::new(31.5, 1.8, -18.5),
        Vec3::new(29.0, 1.4, -15.0),
    ),
    key(
        7.0,
        Vec3::new(29.5, 2.6, -10.0),
        Vec3::new(26.0, 1.5, -14.0),
    ),
    key(
        14.0,
        Vec3::new(26.5, 4.0, -4.5),
        Vec3::new(21.5, 1.2, -15.0),
    ),
];

/// Shot 3, "Skirmish": slow arc along the south edge of the arena while the
/// two fighters circle and trade blows.
const SKIRMISH_KEYS: &[CameraKey] = &[
    key(0.0, Vec3::new(11.0, 1.9, 31.5), Vec3::new(2.0, 1.3, 38.0)),
    key(8.0, Vec3::new(2.0, 1.6, 28.5), Vec3::new(2.0, 1.3, 38.5)),
    key(16.0, Vec3::new(-7.5, 2.2, 32.5), Vec3::new(2.5, 1.2, 38.5)),
];

/// Shot 4, "Starfall": near-static dusk framing from the meteor field edge.
/// The look-at starts high on the entry sky, tracks the descent, and settles
/// on the crater glow after impact at the 12 s mark.
const STARFALL_KEYS: &[CameraKey] = &[
    key(
        0.0,
        Vec3::new(-31.0, 2.4, -26.0),
        Vec3::new(-75.0, 30.0, -75.0),
    ),
    key(
        10.0,
        Vec3::new(-32.0, 2.7, -27.5),
        Vec3::new(-60.0, 15.0, -60.0),
    ),
    key(
        13.0,
        Vec3::new(-32.5, 2.9, -28.5),
        Vec3::new(-42.0, 4.5, -38.0),
    ),
    key(
        20.0,
        Vec3::new(-34.5, 3.6, -31.0),
        Vec3::new(-42.0, 2.5, -38.0),
    ),
];

/// Shot 5, "Emberlight": night close-up on the furnace glow, pulling back
/// and rising to the torch-lit cabin under the stars.
const EMBERLIGHT_KEYS: &[CameraKey] = &[
    key(
        0.0,
        Vec3::new(15.8, 1.8, -20.8),
        Vec3::new(17.8, 1.2, -18.6),
    ),
    key(
        8.0,
        Vec3::new(13.5, 3.2, -10.5),
        Vec3::new(21.0, 1.8, -14.0),
    ),
    // Pull-back ends high and slightly north so the birch at (14, -7.5)
    // stays under the lens (verified against a headless capture).
    key(16.0, Vec3::new(7.0, 8.0, -4.5), Vec3::new(22.0, 2.2, -14.5)),
];

pub const SHOTS: &[Shot] = &[
    Shot {
        name: "Daybreak",
        world_time_hours: 6.8,
        camera: CameraPath {
            keys: DAYBREAK_KEYS,
        },
        track_meteor: false,
    },
    Shot {
        name: "Harvest",
        world_time_hours: 9.3,
        camera: CameraPath { keys: HARVEST_KEYS },
        track_meteor: false,
    },
    Shot {
        name: "Homestead",
        world_time_hours: 13.5,
        camera: CameraPath {
            keys: HOMESTEAD_KEYS,
        },
        track_meteor: false,
    },
    Shot {
        name: "Skirmish",
        // Same daylight as the opening shots (owner call).
        world_time_hours: 13.5,
        camera: CameraPath {
            keys: SKIRMISH_KEYS,
        },
        track_meteor: false,
    },
    Shot {
        name: "Starfall",
        // Same daylight as the opening shots (owner call).
        world_time_hours: 13.5,
        camera: CameraPath {
            keys: STARFALL_KEYS,
        },
        track_meteor: true,
    },
    Shot {
        name: "Emberlight",
        // Late dusk rather than full night: the torch and furnace glow still
        // read as the light sources, but the cabin silhouette keeps shape.
        world_time_hours: 21.0,
        camera: CameraPath {
            keys: EMBERLIGHT_KEYS,
        },
        track_meteor: false,
    },
];

/// Shot lookup by wire index. Out-of-range indices (a newer server playing a
/// longer script than this client knows) return `None`; the client just
/// leaves the camera on the last good pose.
pub fn shot(index: usize) -> Option<&'static Shot> {
    SHOTS.get(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_sane() {
        assert!(!SHOTS.is_empty());
        assert!(METEOR_SHOT_INDEX < SHOTS.len());
        for (index, shot) in SHOTS.iter().enumerate() {
            assert!(
                shot.camera.keys.len() >= 2,
                "shot {index} needs at least two camera keys"
            );
            assert_eq!(
                shot.camera.keys[0].t, 0.0,
                "shot {index} must start its keys at t = 0"
            );
            let mut prev = -1.0;
            for k in shot.camera.keys {
                assert!(k.t > prev, "shot {index} keys must strictly increase in t");
                prev = k.t;
            }
            assert!(
                shot.duration_seconds() >= 5.0,
                "shot {index} is too short to cut"
            );
            assert!(
                (0.0..24.0).contains(&shot.world_time_hours),
                "shot {index} world time out of range"
            );
        }
        // The meteor shot must run long enough for warning + impact + dust.
        let starfall = &SHOTS[METEOR_SHOT_INDEX];
        assert!(starfall.duration_seconds() > METEOR_WARNING_SECONDS + 4.0);
        // The skirmish kill must land inside its shot with room to breathe.
        assert!(SKIRMISH_SHOT_INDEX < SHOTS.len());
        assert!(SHOTS[SKIRMISH_SHOT_INDEX].duration_seconds() > SKIRMISH_KILL_SECONDS + 2.0);
    }
}
