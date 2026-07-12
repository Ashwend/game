//! Client state for the radial wheel menu and the building plan's piece
//! selection.
//!
//! The wheel is a hold-to-open overlay: holding the trigger (right mouse
//! for the plan/hammer/door wheels, the pickup key for the sleeping-bag
//! wheel) opens it, mouse motion picks a sector, and a left click commits
//! the highlighted option. Releasing the trigger closes the wheel; only
//! wheels that opt into `commit_on_release` (the building plan's piece
//! picker, whose options just flip local state) also commit the
//! highlighted option on release, wheels whose options cause real writes
//! (demolish, upgrade) keep selection as an explicit click. While a
//! wheel is open the camera is frozen (see `mouse_look_system`)
//! and swings are suppressed, but gameplay
//! simulation keeps running per the "gameplay never pauses" invariant:
//! the wheel is not a menu overlay, it never touches
//! `gameplay_accepts_controls`.

use bevy::prelude::*;

use crate::{building::BuildingPiece, protocol::DeployedEntityId};

/// Pointer travel (pixels of accumulated mouse delta) before a sector
/// counts as selected. Inside this radius the wheel shows no selection
/// and releasing commits nothing.
pub(crate) const WHEEL_DEADZONE_PX: f32 = 24.0;
/// Cap on the accumulated pointer length so long drags stay responsive
/// when the player swings back toward another sector.
pub(crate) const WHEEL_POINTER_MAX_PX: f32 = 140.0;
/// Hold time on the pickup key before a tap (pick the bag up) becomes a
/// hold (open the bag wheel).
pub(crate) const PICKUP_HOLD_WHEEL_SECS: f32 = 0.35;

/// Which physical input keeps a wheel open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WheelTrigger {
    RightMouse,
    PickupKey,
}

/// What committing a wheel option does. Baked into the option when the
/// wheel opens so commit-time needs no target re-resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WheelAction {
    SelectPiece(BuildingPiece),
    UpgradeBuilding(DeployedEntityId),
    DemolishBuilding(DeployedEntityId),
    ChangeDoorCode(DeployedEntityId),
    /// Door hold-E wheel: pick the door back into inventory (server gates
    /// on claim authorization + knowing the code).
    PickUpDoor(DeployedEntityId),
    RenameBag(DeployedEntityId),
    PickUpBag(DeployedEntityId),
    /// Tool Cupboard hold-E wheel: authorize / deauthorize the local
    /// player, or clear the whole authorized list.
    AuthorizeCupboard(DeployedEntityId),
    DeauthorizeCupboard(DeployedEntityId),
    ClearCupboard(DeployedEntityId),
    /// Explosive-charge hold-E wheel: defuse the live charge (server gates on
    /// reach + claim authorization, refunds half the materials).
    DefuseCharge(DeployedEntityId),
}

#[derive(Debug, Clone)]
pub(crate) struct WheelOption {
    pub(crate) label: String,
    /// Smaller second line (cost, tier, hint). Purely cosmetic.
    pub(crate) detail: Option<String>,
    /// Whether the detail line describes something currently satisfied
    /// (enough materials, you're the builder). `false` renders the line
    /// in warning red. Deliberately does NOT gate committing: the player
    /// may still pick the option and get the server's toast, the colour
    /// is the at-a-glance eligibility readout.
    pub(crate) detail_ok: bool,
    /// Greyed-out options render but can't be committed. Reserved for
    /// structurally impossible picks (e.g. upgrading past the top tier);
    /// merely-ineligible options stay enabled and rely on `detail_ok`.
    pub(crate) enabled: bool,
    /// Draws a small marker dot next to the label (the plan wheel's
    /// currently-selected piece). Painted by the renderer rather than
    /// baked into the label: the UI font has no glyph for bullet
    /// characters, which render as tofu rectangles.
    pub(crate) marked: bool,
    pub(crate) action: WheelAction,
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveWheel {
    /// Centre title ("Build", "Hammer", ...).
    pub(crate) title: String,
    pub(crate) trigger: WheelTrigger,
    pub(crate) options: Vec<WheelOption>,
    /// Accumulated mouse delta since the wheel opened; its direction
    /// picks the sector.
    pub(crate) pointer: Vec2,
    /// Whether releasing the trigger commits the highlighted option
    /// (instead of just closing). Only for wheels whose options are
    /// non-intrusive local choices (the plan's piece picker); wheels
    /// that cause real-world writes keep the explicit left click.
    pub(crate) commit_on_release: bool,
}

impl ActiveWheel {
    /// Index of the sector the pointer currently selects, `None` inside
    /// the deadzone. Sector 0 is centred at 12 o'clock; the rest follow
    /// clockwise.
    pub(crate) fn selected_index(&self) -> Option<usize> {
        if self.options.is_empty() || self.pointer.length() < WHEEL_DEADZONE_PX {
            return None;
        }
        // Screen-space delta: +x right, +y down. Angle measured clockwise
        // from straight up.
        let angle = self.pointer.x.atan2(-self.pointer.y);
        let span = std::f32::consts::TAU / self.options.len() as f32;
        let index = ((angle + span / 2.0).rem_euclid(std::f32::consts::TAU) / span) as usize;
        Some(index.min(self.options.len() - 1))
    }

    /// The action a left click right now would commit (enabled options
    /// only).
    pub(crate) fn selected_action(&self) -> Option<WheelAction> {
        let option = &self.options[self.selected_index()?];
        option.enabled.then_some(option.action)
    }
}

/// What kind of deployable a hold-the-pickup-key gesture targets. Both
/// share the "quick tap = direct action, hold = options wheel" timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickupHoldKind {
    /// Tap picks the bag up, hold opens the rename/pick-up wheel.
    SleepingBag,
    /// Tap toggles your own authorization, hold opens the clear/auth wheel.
    ToolCupboard,
    /// Tap toggles the door open / prompts for the code, hold opens the
    /// pick-up wheel.
    Door,
    /// A live placed charge: there is no useful tap action (a charge is not
    /// interacted with by tapping), so the hold opens the defuse wheel and a
    /// quick tap is a no-op.
    Explosive,
}

/// In-flight pickup-key hold on a deployable: a quick release does the
/// kind's tap action, holding past [`PICKUP_HOLD_WHEEL_SECS`] opens its
/// wheel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PickupHold {
    pub(crate) id: DeployedEntityId,
    pub(crate) kind: PickupHoldKind,
    pub(crate) elapsed: f32,
}

#[derive(Resource, Debug, Default)]
pub(crate) struct WheelMenuState {
    pub(crate) active: Option<ActiveWheel>,
    pub(crate) pickup_hold: Option<PickupHold>,
    /// True for the frame in which the wheel closed. Click consumers
    /// (swing, placement) treat it as still-open so the left click that
    /// committed a wheel option can't double as a swing or a placement.
    pub(crate) closed_this_frame: bool,
}

impl WheelMenuState {
    /// Whether wheel state should block gameplay click/scroll input this
    /// frame: open, or closed so recently the commit click is still in
    /// flight.
    pub(crate) fn blocks_input(&self) -> bool {
        self.active.is_some() || self.closed_this_frame
    }
}

/// Which building piece the plan places. Survives across wheel opens so
/// the player can keep stamping walls without re-picking.
#[derive(Resource, Debug, Clone, Copy)]
pub(crate) struct BuildingPlanState {
    pub(crate) selected_piece: BuildingPiece,
}

impl Default for BuildingPlanState {
    fn default() -> Self {
        Self {
            selected_piece: BuildingPiece::Foundation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(options: usize, pointer: Vec2) -> ActiveWheel {
        ActiveWheel {
            title: "Test".to_owned(),
            trigger: WheelTrigger::RightMouse,
            options: (0..options)
                .map(|index| WheelOption {
                    label: format!("{index}"),
                    detail: None,
                    detail_ok: true,
                    enabled: true,
                    marked: false,
                    action: WheelAction::SelectPiece(BuildingPiece::Foundation),
                })
                .collect(),
            pointer,
            commit_on_release: false,
        }
    }

    #[test]
    fn deadzone_selects_nothing() {
        assert_eq!(wheel(4, Vec2::new(3.0, 3.0)).selected_index(), None);
    }

    #[test]
    fn cardinal_directions_pick_the_expected_sectors() {
        // Four options: 0 = up, 1 = right, 2 = down, 3 = left.
        assert_eq!(wheel(4, Vec2::new(0.0, -100.0)).selected_index(), Some(0));
        assert_eq!(wheel(4, Vec2::new(100.0, 0.0)).selected_index(), Some(1));
        assert_eq!(wheel(4, Vec2::new(0.0, 100.0)).selected_index(), Some(2));
        assert_eq!(wheel(4, Vec2::new(-100.0, 0.0)).selected_index(), Some(3));
    }

    #[test]
    fn disabled_options_cannot_commit() {
        let mut wheel = wheel(2, Vec2::new(0.0, -100.0));
        wheel.options[0].enabled = false;
        assert_eq!(wheel.selected_index(), Some(0));
        assert_eq!(wheel.selected_action(), None);
    }
}
