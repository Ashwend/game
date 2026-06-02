//! Update-available modal + the persistent corner pill.
//!
//! Pure presentation over [`crate::update::UpdateState`]: the modal renders the
//! changelog (markdown, via `egui_commonmark`) and maps the buttons to
//! `UpdateState` actions (download / skip / restart). The corner pill is the
//! always-available re-entry point once the player dismisses the modal.

use bevy_egui::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::{
    protocol::GAME_VERSION,
    update::{UpdateState, UpdateStatus},
};

use super::{
    modal::modal_shell,
    theme::{self, ButtonKind, compact_button},
};

const BUTTON_WIDTH: f32 = 132.0;

/// The changelog modal. Renders only while [`UpdateState::modal_open`] and an
/// update is known; otherwise it animates closed via the shared modal shell.
pub(super) fn update_modal(
    ctx: &egui::Context,
    update: &mut UpdateState,
    cache: &mut CommonMarkCache,
) {
    let open = update.modal_open && update.available.is_some();

    // Snapshot the read-only bits so the body closure can borrow `update`
    // mutably for the button actions without aliasing.
    let version = update.latest_version().unwrap_or_default().to_owned();
    let changelog = update.changelog().to_owned();
    let status = update.status.clone();
    let can_self_update = update.can_self_update();

    let output = modal_shell(
        ctx,
        "update_modal",
        open,
        460.0,
        640.0,
        |ui, _choice: &mut Option<()>| {
            ui.label(theme::section("Update available"));
            ui.add_space(4.0);
            ui.label(theme::muted(format!(
                "You're on v{GAME_VERSION}. Latest is v{version}."
            )));
            ui.add_space(12.0);

            egui::ScrollArea::vertical()
                .max_height(320.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if changelog.trim().is_empty() {
                        ui.label(theme::muted("No release notes."));
                    } else {
                        CommonMarkViewer::new().show(ui, cache, &changelog);
                    }
                });

            ui.add_space(16.0);
            render_actions(ui, update, &status, can_self_update);
        },
    );

    // Clicking the scrim is a soft dismiss (keep the pill), matching how the
    // other overlays treat outside clicks. Never dismiss mid-download/apply.
    if output.clicked_outside
        && !matches!(
            status,
            UpdateStatus::Downloading { .. } | UpdateStatus::Applying
        )
    {
        update.dismiss_modal();
    }
}

fn render_actions(
    ui: &mut egui::Ui,
    update: &mut UpdateState,
    status: &UpdateStatus,
    can_self_update: bool,
) {
    match status {
        UpdateStatus::Downloading { received, total } => {
            let progress = egui::ProgressBar::new(download_fraction(*received, *total))
                .show_percentage()
                .animate(total.is_none());
            ui.add(progress);
            ui.add_space(4.0);
            ui.label(theme::muted("Downloading update…"));
        }
        UpdateStatus::Ready => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if compact_button(ui, "Restart & update", ButtonKind::Primary, BUTTON_WIDTH)
                    .clicked()
                {
                    update.request_apply();
                }
                if compact_button(ui, "Later", ButtonKind::Secondary, 92.0).clicked() {
                    update.dismiss_modal();
                }
            });
        }
        UpdateStatus::Applying => {
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new());
                ui.add_space(6.0);
                ui.label(theme::muted("Restarting to apply the update…"));
            });
        }
        UpdateStatus::Failed(message) => {
            ui.label(egui::RichText::new(message).color(egui::Color32::from_rgb(235, 130, 130)));
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if compact_button(ui, "Open download page", ButtonKind::Primary, BUTTON_WIDTH)
                    .clicked()
                {
                    update.open_download_page();
                    update.dismiss_modal();
                }
                if compact_button(ui, "Close", ButtonKind::Secondary, 92.0).clicked() {
                    update.dismiss_modal();
                }
            });
        }
        // Available (or any state with the modal open and nothing in flight).
        _ => {
            let primary_label = if can_self_update {
                "Update now"
            } else {
                "Open download page"
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if compact_button(ui, primary_label, ButtonKind::Primary, BUTTON_WIDTH).clicked() {
                    update.begin_download();
                }
                if compact_button(ui, "Skip this version", ButtonKind::Secondary, BUTTON_WIDTH)
                    .clicked()
                {
                    update.skip();
                }
                if compact_button(ui, "Later", ButtonKind::Secondary, 72.0).clicked() {
                    update.dismiss_modal();
                }
            });
        }
    }
}

fn download_fraction(received: u64, total: Option<u64>) -> f32 {
    match total {
        Some(total) if total > 0 => (received as f32 / total as f32).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// A small "Update available" pill anchored to a screen corner, shown on menu
/// screens whenever an update is pending. Clicking it re-opens the modal.
pub(super) fn update_corner_pill(ctx: &egui::Context, update: &mut UpdateState) {
    if !update.has_update() || update.modal_open {
        return;
    }
    let label = match &update.status {
        UpdateStatus::Ready => "Update ready to install".to_owned(),
        UpdateStatus::Downloading { received, total } => {
            let pct = (download_fraction(*received, *total) * 100.0).round() as u32;
            format!("Updating… {pct}%")
        }
        _ => match update.latest_version() {
            Some(version) => format!("Update available: v{version}"),
            None => "Update available".to_owned(),
        },
    };

    egui::Area::new("update_corner_pill".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::RIGHT_TOP, [-16.0, 16.0])
        .show(ctx, |ui| {
            if compact_button(ui, &label, ButtonKind::Primary, 0.0).clicked() {
                update.open_modal();
            }
        });
}

/// Renders a pause-menu row that opens the update modal when an update is
/// pending in-game (the corner pill is suppressed over the HUD).
pub(super) fn pause_update_row(ui: &mut egui::Ui, update: &mut UpdateState) {
    if !update.has_update() {
        return;
    }
    let label = match &update.status {
        UpdateStatus::Ready => "Update ready to install".to_owned(),
        _ => "Update available".to_owned(),
    };
    if super::menu_button(ui, &label).clicked() {
        update.open_modal();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_fraction_handles_unknown_and_zero_totals() {
        assert_eq!(download_fraction(0, None), 0.0);
        assert_eq!(download_fraction(50, Some(0)), 0.0);
        assert_eq!(download_fraction(50, Some(100)), 0.5);
        // Never overshoots if a server lies about Content-Length.
        assert_eq!(download_fraction(200, Some(100)), 1.0);
    }
}
