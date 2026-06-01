use bevy_egui::egui;

use crate::{
    app::state::{AuthFlow, MenuState, WorkosAuth},
    workos_login::{ScreenHint, begin_login},
};

use super::{
    danger_menu_button, menu_button, primary_menu_button,
    theme::{self, MENU_BUTTON_WIDTH, MENU_WIDTH},
};

/// What the login splash should render this frame, snapshotted so the closure
/// doesn't borrow `AuthFlow` (we mutate it after, to start a login).
enum LoginView {
    LoggedOut { error: Option<String> },
    Busy(&'static str),
}

/// The auth gate shown in place of the title screen until the user is signed
/// in. Drives the `LoggedOut → Authenticating` transition; the spinner states
/// (`Verifying`/`Authenticating`) are advanced by `drive_auth_flow_system`.
/// Returns without drawing once `Authenticated`.
pub(super) fn login_overlay_ui(
    ctx: &egui::Context,
    auth: &mut AuthFlow,
    workos: &WorkosAuth,
    menu: &mut MenuState,
) {
    let view = match &*auth {
        AuthFlow::Authenticated => return,
        AuthFlow::LoggedOut { error } => LoginView::LoggedOut {
            error: error.clone(),
        },
        AuthFlow::Verifying(_) => LoginView::Busy("Signing you in…"),
        AuthFlow::Authenticating(_) => {
            LoginView::Busy("Finish signing in in your browser, then return here.")
        }
    };

    theme::screen_scrim(ctx, "login_scrim", 170);

    let mut start: Option<ScreenHint> = None;
    let mut quit = false;
    egui::Area::new("login_overlay".into())
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -20.0])
        .show(ctx, |ui| {
            ui.set_width(MENU_WIDTH);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Label::new(theme::title("ASHWEND", 78.0))
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
                ui.add_space(20.0);
                let panel = theme::panel_frame().inner_margin(egui::Margin::same(24));
                panel.show(ui, |ui| {
                    ui.set_width(MENU_BUTTON_WIDTH);
                    ui.vertical_centered(|ui| match view {
                        LoginView::LoggedOut { error } => {
                            ui.label(theme::muted("Sign in to play."));
                            ui.add_space(14.0);
                            if primary_menu_button(ui, "Sign in").clicked() {
                                start = Some(ScreenHint::SignIn);
                            }
                            if menu_button(ui, "Create account").clicked() {
                                start = Some(ScreenHint::SignUp);
                            }
                            if danger_menu_button(ui, "Quit").clicked() {
                                quit = true;
                            }
                            if let Some(error) = error {
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(error)
                                        .color(egui::Color32::from_rgb(231, 132, 110)),
                                );
                            }
                        }
                        LoginView::Busy(message) => {
                            ui.add_space(6.0);
                            ui.add(egui::Spinner::new().size(28.0));
                            ui.add_space(14.0);
                            ui.label(theme::muted(message));
                        }
                    });
                });
            });
        });

    if let Some(hint) = start {
        *auth = AuthFlow::Authenticating(begin_login(&workos.0, hint));
    }
    if quit {
        menu.quit_requested = true;
    }
}
