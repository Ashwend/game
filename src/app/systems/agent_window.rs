//! Dev-only: keep an agent-driven launch from stealing focus on macOS.
//!
//! winit, in `applicationDidFinishLaunching`, sets the app's activation policy
//! to `Regular` and calls `activateIgnoringOtherApps`, so a freshly launched
//! client becomes the frontmost app and grabs focus, regardless of the window's
//! `focused`/`with_active(false)` flag (that flag only governs window-level key
//! status, not app activation). For an agent session we don't want that: the
//! window should come up in the background without interrupting whatever the
//! user is doing.
//!
//! There is no Bevy seam to set winit's activation policy before launch, so we
//! correct it on the first frame: drop the process to an *accessory* app (no
//! Dock icon, its windows never take focus) and resign the active status winit
//! grabbed, handing focus back to the previous app. macOS-only and gated on
//! `debug_assertions`, so it never ships.

use bevy::ecs::system::NonSendMarker;

/// Startup system, registered only for agent-driven dev sessions. The
/// [`NonSendMarker`] pins it to the main thread, which is required to touch
/// `NSApplication`.
pub(crate) fn relinquish_macos_focus_system(_main_thread: NonSendMarker) {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;

    // Belt-and-suspenders: the NonSendMarker already guarantees the main thread,
    // but `MainThreadMarker::new` returning `None` would mean we're not, so bail
    // rather than risk an off-main-thread AppKit call.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // `deactivate` is the documented way to give up active (frontmost) status;
    // paired with the accessory policy, focus returns to the previous app.
    // SAFETY: called on the main thread (guaranteed by the NonSendMarker and
    // the MainThreadMarker check above), which is AppKit's only requirement.
    #[allow(deprecated)]
    unsafe {
        app.deactivate();
    }
}
