//! Shared definitions for the scripted marketing cinematic.
//!
//! The cinematic is a server-orchestrated sequence of camera shots recorded
//! from a hand-authored "stage" world (`MapType::Cinematic`). This module is
//! the single source of truth both sides read:
//!
//! - **Stage layout** (`layout`): the pinned world seed and dims, the clear
//!   zones where procedural scatter is suppressed, the authored resource-node
//!   placements, the pre-built base compound, and the dummy-actor roster. The
//!   server consumes all of it (worldgen injection plus the orchestrator's
//!   actor scripts); the client consumes none of it directly, every stage
//!   object reaches clients through ordinary replication.
//! - **Shot script** (`script`): the ordered shot list with per-shot duration
//!   and time-of-day, plus the countdown / intermission timings. The server
//!   ticks this timeline and broadcasts one `ServerMessage::Cinematic` cue per
//!   phase transition; the client renders countdown overlays and drives the
//!   detached camera from the same table, so the two sides only need the shot
//!   index on the wire.
//! - **Camera paths** (`camera`): keyframed eye / look-at splines evaluated
//!   client-side each frame while a shot plays.
//!
//! Playback is started with the admin chat command `/cinematic` (see
//! `crate::server::cinematic`); the client-side director lives in
//! `crate::app` (camera system, countdown overlay, control gating). See
//! docs/cinematic.md for the full architecture and the recording workflow.

pub mod camera;
pub mod layout;
pub mod script;

pub use camera::{CameraKey, CameraPath};
pub use layout::{
    ActorRole, ActorSpec, ArmorLoadout, CINEMATIC_SEED, STAGE_ACTORS, StagePropKind, StageZone,
    cinematic_dims, stage_exclusion_footprints,
};
pub use script::{
    COUNTDOWN_SECONDS, INIT_SECONDS, INTERMISSION_SECONDS, METEOR_SHOT_INDEX,
    METEOR_WARNING_SECONDS, SHOTS, Shot, shot,
};
