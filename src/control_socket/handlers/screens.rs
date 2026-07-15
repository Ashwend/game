//! Screen and overlay handlers: menu navigation, the inventory / crafting
//! panels, and the world-map overlay (open state, viewport, markers) whose
//! focus-gated hotkeys a headless window can't receive.

use anyhow::{Context, Result, bail};

use super::HandlerContext;
use crate::{app::state::Screen, protocol::ClientMessage};

pub(super) fn set_screen(ctx: &mut HandlerContext, screen: String) -> Result<String> {
    ctx.menu.screen = parse_screen(&screen)?;
    Ok(format!("screen set to {:?}", ctx.menu.screen))
}

pub(super) fn set_inventory_open(
    ctx: &mut HandlerContext,
    open: bool,
    admin_tab: bool,
) -> Result<String> {
    ctx.menu.inventory_open = open;
    ctx.inventory_ui.admin_tab = open && admin_tab;
    Ok(format!("inventory_open = {open} (admin_tab = {admin_tab})"))
}

pub(super) fn set_crafting_open(ctx: &mut HandlerContext, open: bool) -> Result<String> {
    ctx.menu.crafting_open = open;
    if open {
        ctx.menu.inventory_open = false;
    }
    Ok(format!("crafting_open = {open}"))
}

pub(super) fn set_world_map_open(ctx: &mut HandlerContext, open: bool) -> Result<String> {
    ctx.menu.world_map_open = open;
    if open && let Some(session) = ctx.runtime.session.as_mut() {
        // Pull the terrain + markers so the overlay isn't stuck on
        // "Loading map..." in the screenshot.
        session.send(ClientMessage::RequestWorldMap)?;
    }
    Ok(format!("world_map_open = {open}"))
}

pub(super) fn add_world_map_marker(ctx: &mut HandlerContext, x: f32, z: f32) -> Result<String> {
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::WorldMapMarker(
        crate::protocol::WorldMapMarkerCommand::Add { x, z },
    ))?;
    Ok(format!("add marker queued at [{x:.1}, {z:.1}]"))
}

pub(super) fn set_world_map_view(
    ctx: &mut HandlerContext,
    zoom: f32,
    center_x: f32,
    center_z: f32,
) -> Result<String> {
    if !zoom.is_finite() || !center_x.is_finite() || !center_z.is_finite() {
        bail!("zoom/center must be finite");
    }
    ctx.world_map_ui.zoom = zoom;
    ctx.world_map_ui.center = Some((center_x, center_z));
    Ok(format!(
        "world map view: zoom {zoom:.2}, centre [{center_x:.1}, {center_z:.1}]"
    ))
}

/// Map an agent-supplied screen name to a [`Screen`]. Tolerant of case and of
/// `_`/`-`/space separators so `"main_menu"`, `"MainMenu"`, and `"in game"` all
/// work.
fn parse_screen(raw: &str) -> Result<Screen> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['_', '-', ' '], "");
    Ok(match normalized.as_str() {
        "mainmenu" | "menu" | "main" => Screen::MainMenu,
        "worlds" => Screen::Worlds,
        "multiplayer" => Screen::Multiplayer,
        "options" => Screen::Options,
        "ingame" | "game" => Screen::InGame,
        other => bail!("unknown screen '{other}'"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_screen_accepts_aliases_and_separators() {
        assert!(matches!(parse_screen("main_menu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("MainMenu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("  worlds "), Ok(Screen::Worlds)));
        assert!(matches!(parse_screen("in game"), Ok(Screen::InGame)));
        assert!(matches!(
            parse_screen("multiplayer"),
            Ok(Screen::Multiplayer)
        ));
        assert!(parse_screen("nonsense").is_err());
    }
}
