//! Dev tab: live shader / pipeline toggles for isolating visual glitches.
//!
//! Debug-only (the tab is hidden in release builds, see `OptionsTab::ALL`). Every
//! toggle defaults ON; flipping one OFF disables that stage so you can see what it
//! was contributing. The toon / grass toggles drive a `dev_flags` shader uniform
//! (no pipeline recompile); the lighting toggles force a camera / light component
//! off. Applied by `apply_dev_render_settings` (`systems/dev_render.rs`).

use bevy_egui::egui;

use crate::app::{state::ClientSettings, ui::theme};

use super::widgets::{
    caption, checkbox_with_click_sound, section_label, setting_row, value_slider_row,
};

pub(super) fn render(ui: &mut egui::Ui, settings: &mut ClientSettings) {
    theme::inset_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(section_label("Dev overrides"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::compact_button(
                    ui,
                    "Reset to player defaults",
                    theme::ButtonKind::Secondary,
                    190.0,
                )
                .clicked()
                {
                    // Back to exactly what a shipped player sees: every toggle on,
                    // every lighting slider at its production value. Only this tab,
                    // unlike the header "Reset" which wipes all settings.
                    settings.dev = Default::default();
                }
            });
        });
        ui.label(caption(
            "Resets every toggle and slider on THIS tab to the shipped player \
             defaults. Other tabs are left alone.",
        ));
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Toon material (props, ore, trees, tools)"));
        ui.add_space(6.0);
        setting_row(ui, "Cel posterize", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.cel_posterize, "Enabled");
        });
        setting_row(ui, "Band-edge anti-alias", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.cel_band_aa, "Enabled");
        });
        setting_row(ui, "Ink / outline edge", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.ink_edge, "Enabled");
        });
        setting_row(ui, "Saturation lift", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.saturation, "Enabled");
        });
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Grass"));
        ui.add_space(6.0);
        setting_row(ui, "Cel posterize", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.grass_cel, "Enabled");
        });
        setting_row(ui, "Wind animation", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.grass_wind, "Enabled");
        });
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Lighting / atmosphere"));
        ui.add_space(6.0);
        setting_row(ui, "Bloom", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.bloom, "Enabled");
        });
        setting_row(ui, "Sun shadows", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.sun_shadows, "Enabled");
        });
        setting_row(ui, "Soft shadows (PCSS)", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.soft_shadows, "Enabled");
        });
        setting_row(ui, "Atmosphere ambient (IBL)", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.atmosphere_ibl, "Enabled");
        });
        setting_row(ui, "Distance fog", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.fog, "Enabled");
        });
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Title screen"));
        ui.add_space(6.0);
        setting_row(ui, "Backdrop time slider", |ui| {
            checkbox_with_click_sound(ui, &mut settings.dev.backdrop_time_slider, "Shown");
        });
        ui.label(caption(
            "Shows a time-of-day scrubber on the title screen for re-tuning the \
             pinned backdrop time, then bake the value into MENU_BACKDROP_SECONDS. \
             Off for a clean menu.",
        ));
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.label(section_label("Lighting tuning (live)"));
        ui.add_space(2.0);
        ui.label(caption(
            "Drag to tune in-game, then report the values back. Use /time HHMM \
             and /time-speed 0 to freeze the sun while comparing. Defaults are the \
             shipped values.",
        ));
        ui.add_space(6.0);
        let light = &mut settings.dev.lighting;
        // Sun peak illuminance: the daytime brightness. ~2.5k-5k is a sunny day at
        // this exposure (see DevLighting docs); range gives headroom either way.
        value_slider_row(
            ui,
            "Sun peak illuminance",
            &mut light.sun_peak_illuminance,
            1_000.0..=15_000.0,
            50.0,
            0,
        );
        // Atmosphere IBL ambient: daytime sky-bounce fill (0 = none; gated off by
        // the Atmosphere ambient toggle above regardless of this value).
        value_slider_row(
            ui,
            "Atmosphere ambient (IBL)",
            &mut light.atmosphere_ibl_intensity,
            0.0..=2.0,
            0.01,
            2,
        );
        // Midday cap elevation: sun height above which midday brightness is held
        // flat. Lower caps harder; 1.0 effectively disables the cap.
        value_slider_row(
            ui,
            "Midday cap elevation",
            &mut light.midday_cap_elevation,
            0.35..=1.0,
            0.001,
            3,
        );
        // Above-plateau droop exponent: higher dims the high sun more.
        value_slider_row(
            ui,
            "Midday droop exponent",
            &mut light.overhead_exponent,
            0.0..=1.5,
            0.01,
            2,
        );
    });

    ui.add_space(10.0);

    theme::inset_frame().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(section_label("Combat feel (live)"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::compact_button(ui, "Neutral", theme::ButtonKind::Secondary, 90.0)
                    .clicked()
                {
                    // Snap every combat slider back to its neutral default (all
                    // scales 1.0, the offset 0.0) so a tuning pass can reset to
                    // the shipped feel without touching the other Dev sliders.
                    settings.dev.combat = Default::default();
                }
            });
        });
        ui.add_space(2.0);
        ui.label(caption(
            "Tune weapon feel live (ideally in a two-client session), then bake the \
             values into game_balance.rs. Defaults are neutral, so a release build \
             feels identical. The client sliders apply instantly; knockback is \
             server state and needs the chat command below.",
        ));
        ui.add_space(6.0);
        let combat = &mut settings.dev.combat;
        // Swing duration multiplier: stretches / compresses the whole local swing
        // animation (and its impact cue) without drifting the contact frame.
        value_slider_row(
            ui,
            "Swing duration scale",
            &mut combat.swing_duration_scale,
            0.5..=2.0,
            0.01,
            2,
        );
        // Impact fraction offset: shifts where in the swing the contact cue fires;
        // the consumer clamps the result to a sane window.
        value_slider_row(
            ui,
            "Impact fraction offset",
            &mut combat.impact_fraction_offset,
            -0.2..=0.2,
            0.01,
            2,
        );
        // Camera kick magnitude: scales both the pitch punch and the drop; 0
        // disables the kick entirely.
        value_slider_row(
            ui,
            "Camera kick magnitude scale",
            &mut combat.kick_magnitude_scale,
            0.0..=3.0,
            0.01,
            2,
        );
        // Camera kick duration: how long the punch lingers before it settles.
        value_slider_row(
            ui,
            "Camera kick duration scale",
            &mut combat.kick_duration_scale,
            0.5..=2.0,
            0.01,
            2,
        );
        // Hit-stop: the brief attacker-side viewmodel freeze on a confirmed
        // player hit. Scales the shipped window; 0 disables it, up to 3x for
        // tuning a heavier committed feel.
        value_slider_row(
            ui,
            "Hit-stop scale",
            &mut combat.hit_stop_scale,
            0.0..=3.0,
            0.01,
            2,
        );
        // Knockback scale: server state, so the slider only remembers the value.
        // The hint below spells out the chat command that applies it live.
        value_slider_row(
            ui,
            "Knockback scale",
            &mut combat.knockback_scale,
            0.0..=3.0,
            0.01,
            2,
        );
        ui.add_space(2.0);
        ui.label(caption(&format!(
            "Knockback is authoritative server state, so this slider does not apply \
             on its own. Run  /knockback-scale {:.2}  in chat to apply it (admin \
             only; resets to 1.0 on server restart).",
            combat.knockback_scale,
        )));
    });
}
