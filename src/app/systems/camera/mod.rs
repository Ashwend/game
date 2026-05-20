//! Camera systems split by concern:
//!
//! - [`effects`] owns the per-frame motion modifiers (impact kick, head bob,
//!   sprint FOV, landing dip) as plain `Resource`s.
//! - [`follow`] owns the in-game first-person follow system that consumes
//!   those effects and the latest predicted-pose snapshot.
//! - [`menu_backdrop`] owns the menu-screen panning camera plus its
//!   post-processing toggles for the depth-of-field backdrop look.

mod effects;
mod follow;
mod menu_backdrop;

pub(crate) use effects::{CameraImpactKick, CameraMotionEffects};
pub(crate) use follow::camera_follow_system;
pub(crate) use menu_backdrop::menu_backdrop_camera_system;
