use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    app::{
        scene::{MainCamera, NetworkPlayer, PLAYER_HEAD_TOP_LOCAL_Y},
        voice::VoiceState,
    },
    protocol::{ClientId, MAX_HEALTH},
    server::{Player, PlayerLifecycle, PlayerPublic},
};

/// Hard cutoff: anything farther than this is skipped entirely. Tuned to a
/// conversational range — close enough that you'd realistically read a
/// nameplate or chat line and not have it cluttering the world otherwise.
const PEER_DRAW_DISTANCE_METERS: f32 = 7.0;
/// Distance at which the label starts fading toward invisible, so it
/// dissolves smoothly across the last 1.5 m instead of popping out.
const PEER_FADE_START_METERS: f32 = 5.5;
/// Inset (logical pixels) from the viewport edges before we stop drawing.
/// When the projected head position crosses this margin we hide the label
/// entirely rather than draw it half-clipped against the screen border.
const PEER_VIEWPORT_INSET_PX: f32 = 12.0;
/// Vertical clearance above the head where the name strip is anchored, so
/// the visor never overlaps the text.
const NAMETAG_HEAD_CLEARANCE_M: f32 = 0.18;

const NAMETAG_WIDTH: f32 = 168.0;
const NAMETAG_HEIGHT: f32 = 36.0;
const HEALTH_BAR_HEIGHT: f32 = 4.0;
const CHAT_BUBBLE_MAX_WIDTH: f32 = 240.0;
const CHAT_BUBBLE_MIN_WIDTH: f32 = 80.0;
const CHAT_BUBBLE_GAP_PX: f32 = 8.0;

pub(crate) struct PeerOverlay<'world> {
    pub(crate) camera: Option<(&'world Camera, GlobalTransform)>,
    pub(crate) peers: Vec<PeerOverlayEntry<'world>>,
}

pub(crate) struct PeerOverlayEntry<'world> {
    pub(crate) head_world: Vec3,
    pub(crate) client_id: ClientId,
    pub(crate) public: &'world PlayerPublic,
    /// `true` when the peer has spoken within roughly the last 200 ms.
    /// Drives the small microphone glyph that appears beside the
    /// nameplate so listeners can tell who's talking.
    pub(crate) speaking: bool,
}

/// Draws floating name+health labels and chat bubbles above remote players.
/// Always-visible label, optional bubble that appears for a few seconds after
/// the player sends chat (driven by the server's `chat_bubble` snapshot field).
///
/// Each label is screen-projected from the player's head world position, so
/// it tracks the camera automatically — billboard behaviour with no extra
/// orientation math.
pub(super) fn peer_overlay_ui(ctx: &egui::Context, overlay: PeerOverlay<'_>) {
    let Some((camera, camera_transform)) = overlay.camera else {
        return;
    };
    let camera_forward = camera_transform.forward().as_vec3();
    let camera_origin = camera_transform.translation();
    // egui's screen rect is in the same logical-pixel space that
    // `world_to_viewport` returns, so we can bounds-check directly. Pulled
    // once per frame rather than per peer.
    let visible_rect = ctx.content_rect().shrink(PEER_VIEWPORT_INSET_PX);

    for peer in overlay.peers {
        let to_peer = peer.head_world - camera_origin;
        // Reject anything behind the camera or past the cull radius.
        if to_peer.dot(camera_forward) <= 0.0 {
            continue;
        }
        let distance = to_peer.length();
        if distance > PEER_DRAW_DISTANCE_METERS {
            continue;
        }
        let Ok(screen) = camera.world_to_viewport(&camera_transform, peer.head_world) else {
            continue;
        };
        // Hide labels whose anchor has drifted off-screen instead of letting
        // egui clip them at the edge — half-clipped nameplates floating in
        // the corner read as a UI glitch.
        if !visible_rect.contains(egui::pos2(screen.x, screen.y)) {
            continue;
        }
        draw_peer_label(
            ctx,
            screen,
            distance,
            peer.client_id,
            peer.public,
            peer.speaking,
        );
    }
}

fn draw_peer_label(
    ctx: &egui::Context,
    screen: Vec2,
    distance: f32,
    client_id: ClientId,
    public: &PlayerPublic,
    speaking: bool,
) {
    let id = egui::Id::new(("peer_overlay", client_id));
    let fade = distance_fade(distance);

    // `anchor()` and `fixed_pos()` conflict — `anchor()` pins the area to a
    // screen edge and ignores the fixed position. We want the area's
    // bottom-center to sit at `screen` (above the player's head), so we use
    // `pivot(CENTER_BOTTOM) + fixed_pos(screen)` instead. Foreground order
    // keeps the labels above the world but below modal dialogs and the
    // loading splash (Tooltip order).
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(false)
        .movable(false)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(egui::pos2(screen.x, screen.y))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(text) = public.chat_bubble.as_deref()
                    && !text.is_empty()
                {
                    chat_bubble(ui, text, fade);
                    ui.add_space(CHAT_BUBBLE_GAP_PX);
                }
                nametag(ui, public, fade, speaking);
            });
        });
}

fn nametag(ui: &mut egui::Ui, public: &PlayerPublic, fade: f32, speaking: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(NAMETAG_WIDTH, NAMETAG_HEIGHT),
        egui::Sense::hover(),
    );

    let bg_alpha = scaled(186, fade);
    let stroke_alpha = scaled(120, fade);
    ui.painter().rect(
        rect,
        4,
        egui::Color32::from_rgba_unmultiplied(8, 10, 14, bg_alpha),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(120, 138, 160, stroke_alpha),
        ),
        egui::StrokeKind::Inside,
    );

    let name_text_color = if public.is_admin {
        egui::Color32::from_rgb(255, 214, 130)
    } else {
        egui::Color32::from_rgb(232, 238, 246)
    };
    let name_rect = egui::Rect::from_min_max(
        rect.min + egui::vec2(8.0, 4.0),
        egui::pos2(rect.right() - 8.0, rect.top() + 20.0),
    );
    let text_rect = ui.painter().text(
        name_rect.center(),
        egui::Align2::CENTER_CENTER,
        truncated_name(&public.name, 22),
        egui::FontId::new(12.5, egui::FontFamily::Proportional),
        with_alpha(name_text_color, scaled(u8::MAX, fade)),
    );

    // Paint the speaking dot AFTER the name so we can anchor it to the
    // actual rendered text rect — that's how we keep it immediately to the
    // left of the name (with a small gap) regardless of how long the
    // player's display name is.
    if speaking {
        draw_voice_indicator(ui, text_rect, fade);
    }

    let bar_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 10.0),
        egui::pos2(rect.right() - 8.0, rect.bottom() - 10.0 + HEALTH_BAR_HEIGHT),
    );
    let fraction = (public.health / MAX_HEALTH).clamp(0.0, 1.0);
    let fill_rect = egui::Rect::from_min_max(
        bar_rect.min,
        egui::pos2(
            bar_rect.left() + bar_rect.width() * fraction,
            bar_rect.bottom(),
        ),
    );
    ui.painter().rect_filled(
        bar_rect,
        1,
        egui::Color32::from_rgba_unmultiplied(30, 32, 38, scaled(220, fade)),
    );
    ui.painter().rect_filled(
        fill_rect,
        1,
        with_alpha(health_fill_color(fraction), scaled(u8::MAX, fade)),
    );
}

fn chat_bubble(ui: &mut egui::Ui, text: &str, fade: f32) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    let font = egui::FontId::new(12.5, egui::FontFamily::Proportional);
    let painter = ui.painter();
    let galley = painter.layout(
        trimmed.to_owned(),
        font,
        with_alpha(
            egui::Color32::from_rgb(236, 240, 248),
            scaled(u8::MAX, fade),
        ),
        CHAT_BUBBLE_MAX_WIDTH - 20.0,
    );
    let bubble_width = (galley.size().x + 20.0).clamp(CHAT_BUBBLE_MIN_WIDTH, CHAT_BUBBLE_MAX_WIDTH);
    let bubble_height = galley.size().y + 14.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(bubble_width, bubble_height),
        egui::Sense::hover(),
    );

    ui.painter().rect(
        rect,
        6,
        egui::Color32::from_rgba_unmultiplied(14, 18, 24, scaled(214, fade)),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(130, 156, 196, scaled(150, fade)),
        ),
        egui::StrokeKind::Inside,
    );

    // Small downward triangle pointing at the nametag below — bubble tail.
    let tail_top_y = rect.bottom();
    let tail_center_x = rect.center().x;
    let tail = vec![
        egui::pos2(tail_center_x - 6.0, tail_top_y - 0.5),
        egui::pos2(tail_center_x + 6.0, tail_top_y - 0.5),
        egui::pos2(tail_center_x, tail_top_y + 6.0),
    ];
    ui.painter().add(egui::Shape::convex_polygon(
        tail,
        egui::Color32::from_rgba_unmultiplied(14, 18, 24, scaled(214, fade)),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(130, 156, 196, scaled(150, fade)),
        ),
    ));

    let text_pos = rect.left_top() + egui::vec2(10.0, 6.0);
    ui.painter().galley(text_pos, galley, egui::Color32::WHITE);
}

/// Paints a small pulsing green dot immediately to the left of the player
/// name to mark a peer who's currently transmitting. Anchored to the
/// name's actual rendered text rect (not the nameplate frame) so the gap
/// stays consistent regardless of name length and the dot sits on the
/// name's vertical centerline. Matches the convention used by
/// Discord / Mumble / most game voice UIs.
///
/// The pulse is a gentle 0.8 Hz sine on alpha so the indicator feels
/// "live" without competing with the player's name for attention.
fn draw_voice_indicator(ui: &egui::Ui, name_text_rect: egui::Rect, fade: f32) {
    /// Pixel gap between the dot's right edge and the first glyph of the
    /// name. Small enough to read as "part of the name", large enough to
    /// not visually touch the text.
    const DOT_TO_NAME_GAP: f32 = 5.0;
    const DOT_RADIUS: f32 = 3.4;
    const HALO_RADIUS: f32 = 6.5;

    let cx = name_text_rect.left() - DOT_TO_NAME_GAP - DOT_RADIUS;
    let cy = name_text_rect.center().y;
    let painter = ui.painter();

    let time = ui.input(|input| input.time);
    let pulse = 0.5 + 0.5 * ((time as f32) * std::f32::consts::TAU * 0.8).sin();

    let green = egui::Color32::from_rgb(110, 220, 130);
    let dot_alpha = scaled((200.0 + pulse * 55.0) as u8, fade);
    let halo_alpha = scaled((50.0 + pulse * 30.0) as u8, fade);

    // Soft halo behind the dot so it reads against bright/busy backgrounds.
    painter.circle_filled(
        egui::pos2(cx, cy),
        HALO_RADIUS,
        with_alpha(green, halo_alpha),
    );
    // Solid dot.
    painter.circle_filled(egui::pos2(cx, cy), DOT_RADIUS, with_alpha(green, dot_alpha));

    // Keep the pulse animation smooth between input events.
    ui.ctx().request_repaint();
}

fn distance_fade(distance: f32) -> f32 {
    if distance <= PEER_FADE_START_METERS {
        return 1.0;
    }
    let span = (PEER_DRAW_DISTANCE_METERS - PEER_FADE_START_METERS).max(0.001);
    let into = (distance - PEER_FADE_START_METERS).clamp(0.0, span);
    1.0 - into / span
}

fn truncated_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() <= max_chars {
        return name.to_owned();
    }
    let mut shortened: String = name.chars().take(max_chars.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

fn scaled(base: u8, fade: f32) -> u8 {
    (f32::from(base) * fade.clamp(0.0, 1.0)).round() as u8
}

fn with_alpha(color: egui::Color32, alpha: u8) -> egui::Color32 {
    let [r, g, b, _] = color.to_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

fn health_fill_color(fraction: f32) -> egui::Color32 {
    if fraction > 0.6 {
        egui::Color32::from_rgb(125, 196, 55)
    } else if fraction > 0.3 {
        egui::Color32::from_rgb(232, 188, 64)
    } else {
        egui::Color32::from_rgb(228, 96, 78)
    }
}

/// Collects the head world positions of each remote player into entries the
/// overlay UI can project. The lookup is keyed by `client_id` so we can pair
/// each `NetworkPlayer` visual entity with the matching replicated
/// `PlayerPublic` — without that pairing, the overlay would have no
/// name/health/bubble to display.
pub(crate) fn collect_peer_overlay_entries<'a>(
    network_players: impl IntoIterator<Item = (&'a NetworkPlayer, &'a GlobalTransform)>,
    replicated_players: impl IntoIterator<
        Item = (&'a Player, &'a PlayerPublic, Option<&'a PlayerLifecycle>),
    >,
    local_client_id: Option<ClientId>,
    voice: &VoiceState,
) -> Vec<PeerOverlayEntry<'a>> {
    // Dead peers get their nameplate suppressed entirely — a tag
    // floating over a tilted-and-fading corpse reads as a UI bug,
    // and a name on a hidden invisible-corpse entity even more so.
    let mut public_by_id: HashMap<ClientId, &PlayerPublic> = replicated_players
        .into_iter()
        .filter(|(player, _, _)| Some(player.client_id) != local_client_id)
        .filter(|(_, _, lifecycle)| !matches!(lifecycle, Some(PlayerLifecycle::Dead { .. })))
        .map(|(player, public, _)| (player.client_id, public))
        .collect();

    network_players
        .into_iter()
        .filter_map(|(player, transform)| {
            let public = public_by_id.remove(&player.client_id)?;
            let translation = transform.translation();
            let head_world =
                translation + Vec3::Y * (PLAYER_HEAD_TOP_LOCAL_Y + NAMETAG_HEAD_CLEARANCE_M);
            Some(PeerOverlayEntry {
                head_world,
                client_id: player.client_id,
                public,
                speaking: voice.is_peer_talking(player.client_id),
            })
        })
        .collect()
}

/// Bevy `SystemParam` that bundles the queries needed to build a
/// [`PeerOverlay`] inside the egui frame.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct PeerOverlayParams<'w, 's> {
    pub(crate) camera: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainCamera>>,
    pub(crate) network_players: Query<'w, 's, (&'static NetworkPlayer, &'static GlobalTransform)>,
    pub(crate) replicated_players: Query<
        'w,
        's,
        (
            &'static Player,
            &'static PlayerPublic,
            Option<&'static PlayerLifecycle>,
        ),
    >,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_fade_starts_full_then_drops_to_zero() {
        assert!((distance_fade(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((distance_fade(PEER_FADE_START_METERS) - 1.0).abs() < f32::EPSILON);
        assert!(distance_fade(PEER_DRAW_DISTANCE_METERS - 0.001) > 0.0);
        assert!(distance_fade(PEER_DRAW_DISTANCE_METERS + 1.0) <= 0.0001);
    }

    #[test]
    fn truncated_name_keeps_short_names_intact() {
        assert_eq!(truncated_name("Tom", 22), "Tom");
        assert!(truncated_name("a".repeat(40).as_str(), 10).ends_with('…'));
    }

    #[test]
    fn health_fill_color_maps_three_tiers() {
        assert_eq!(
            health_fill_color(1.0),
            egui::Color32::from_rgb(125, 196, 55)
        );
        assert_eq!(
            health_fill_color(0.5),
            egui::Color32::from_rgb(232, 188, 64)
        );
        assert_eq!(health_fill_color(0.1), egui::Color32::from_rgb(228, 96, 78));
    }
}
