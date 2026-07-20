//! Camera systems split by concern:
//!
//! - [`effects`] owns the per-frame motion modifiers (impact kick, head bob,
//!   run FOV, ranged draw pinch, landing dip) as plain `Resource`s.
//! - [`follow`] owns the in-game first-person follow system that consumes
//!   those effects and the latest predicted-pose snapshot.
//! - [`viewmodel_fov`] keeps the viewmodel camera's projection in step with
//!   the ranged draw pinch (its FOV is otherwise fixed at spawn).
//! - [`menu_backdrop`] owns the menu-screen panning camera plus its
//!   post-processing toggles for the depth-of-field backdrop look.

mod cinematic;
mod effects;
mod follow;
mod menu_backdrop;
mod viewmodel_fov;

pub(crate) use cinematic::{cinematic_camera_system, tick_cinematic_overlay_system};
pub(crate) use effects::{CameraImpactKick, CameraMotionEffects};
pub(crate) use follow::camera_follow_system;
pub(crate) use menu_backdrop::menu_backdrop_camera_system;
pub(crate) use viewmodel_fov::{VIEWMODEL_BASE_FOV_DEG, sync_viewmodel_fov_system};
