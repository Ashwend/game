//! Dev-only off-screen capture target.
//!
//! When `GAME_HEADLESS_CAPTURE` names a resolution, the client renders the
//! primary camera into an off-screen [`Image`] instead of the window swapchain,
//! and the control socket screenshots that image. Because `bevy_egui` attaches
//! the primary egui context to the first camera (our [`MainCamera`]), pointing
//! that camera's target at the image sends both the 3D scene *and* the full egui
//! UI into it, so a captured frame matches what a player would see.
//!
//! This decouples capture from window visibility. With capture on, the window is
//! created hidden (`visible: false`), which on every platform makes the winit
//! runner fall back to running the app schedule each cycle (the `all_invisible`
//! branch in `bevy_winit`'s `redraw_requested`), so frames keep advancing and
//! the image stays fresh even though nothing is on screen. That removes the
//! macOS "occluded window throttles or closes" failure mode the live-window
//! screenshot path suffers from (see the windowing caveat in
//! `docs/multiplayer-testing.md`).
//!
//! Inert by default: with the env var unset, the window is visible, the camera
//! renders straight to it, and `Screenshot::primary_window()` is used, exactly
//! as before. Shipped builds carry no runtime cost.

use bevy::{
    camera::RenderTarget, image::Image, prelude::*, render::render_resource::TextureFormat,
};

use crate::app::scene::{MainCamera, ViewmodelCamera};

/// Off-screen render target the control socket screenshots when headless capture
/// is enabled. The resource is only inserted in that mode; its absence is what
/// the normal render-to-window path keys off of.
#[derive(Resource, Clone)]
pub(crate) struct HeadlessCapture {
    pub(crate) image: Handle<Image>,
}

impl HeadlessCapture {
    /// Env var that enables headless capture and (optionally) sizes it.
    pub(crate) const ENV: &'static str = "GAME_HEADLESS_CAPTURE";
    const DEFAULT_WIDTH: u32 = 1280;
    const DEFAULT_HEIGHT: u32 = 720;

    /// Capture resolution from [`Self::ENV`], or `None` when capture is off.
    pub(crate) fn resolution_from_env() -> Option<(u32, u32)> {
        parse_resolution(std::env::var(Self::ENV).ok()?.trim())
    }
}

/// Parse the capture resolution. Accepts `WIDTHxHEIGHT` (e.g. `1920x1080`) or a
/// bare truthy value (`1`, `true`, `on`, `yes`) for the default resolution.
/// Returns `None` for an empty or unparseable value so the normal
/// render-to-window path stays untouched.
fn parse_resolution(raw: &str) -> Option<(u32, u32)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if matches!(
        raw.to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    ) {
        return Some((
            HeadlessCapture::DEFAULT_WIDTH,
            HeadlessCapture::DEFAULT_HEIGHT,
        ));
    }
    let (width, height) = raw.split_once(['x', 'X'])?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// Allocate the off-screen image and insert the [`HeadlessCapture`] resource.
/// Call once at startup after `DefaultPlugins` (so `Assets<Image>` exists) when
/// [`HeadlessCapture::resolution_from_env`] returned a size.
pub(crate) fn insert_capture_target(app: &mut App, width: u32, height: u32) {
    let image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    let handle = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    app.insert_resource(HeadlessCapture { image: handle });
}

/// Point the scene cameras at the off-screen capture image. Runs once at startup
/// after the scene (and therefore the cameras) are spawned; a no-op when capture
/// is disabled (the resource is absent). In Bevy 0.18 the render target is a
/// `RenderTarget` component (defaulting to the primary window), so inserting one
/// with the image variant overrides where the camera renders.
///
/// Both the world [`MainCamera`] and the first-person [`ViewmodelCamera`] are
/// redirected: the viewmodel camera composites the held item over the scene
/// (order 1, no colour clear), so it must share the same target or the in-hand
/// tool would render to the hidden window and drop out of every screenshot.
/// The scene cameras whose render target the capture redirect overrides: the
/// world camera and the first-person viewmodel camera that composites over it.
type SceneCameras<'w, 's> = Query<'w, 's, Entity, Or<(With<MainCamera>, With<ViewmodelCamera>)>>;

pub(crate) fn redirect_camera_to_capture(
    mut commands: Commands,
    capture: Option<Res<HeadlessCapture>>,
    cameras: SceneCameras,
) {
    let Some(capture) = capture else {
        return;
    };
    for entity in &cameras {
        commands
            .entity(entity)
            .insert(RenderTarget::Image(capture.image.clone().into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_resolution_accepts_dimensions_and_truthy_aliases() {
        assert_eq!(parse_resolution("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_resolution("800X600"), Some((800, 600)));
        assert_eq!(parse_resolution("  640 x 480 "), Some((640, 480)));
        // Bare truthy values fall back to the default capture resolution.
        assert_eq!(
            parse_resolution("1"),
            Some((
                HeadlessCapture::DEFAULT_WIDTH,
                HeadlessCapture::DEFAULT_HEIGHT
            ))
        );
        assert_eq!(
            parse_resolution("true"),
            Some((
                HeadlessCapture::DEFAULT_WIDTH,
                HeadlessCapture::DEFAULT_HEIGHT
            ))
        );
    }

    #[test]
    fn parse_resolution_rejects_empty_and_malformed() {
        assert_eq!(parse_resolution(""), None);
        assert_eq!(parse_resolution("   "), None);
        assert_eq!(parse_resolution("garbage"), None);
        assert_eq!(parse_resolution("100x"), None);
        assert_eq!(parse_resolution("x100"), None);
        assert_eq!(parse_resolution("0x0"), None);
    }
}
