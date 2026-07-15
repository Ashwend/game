//! Tool taxonomy: [`ToolKind`], the per-tool [`ToolProfile`] (tier, gather
//! payout, cooldown, durability, PvP damage), and the [`HANDS_TOOL`]
//! fallback the gather path substitutes when no tool is held.

use super::visual::ItemModel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ToolKind {
    /// No tool equipped. Synthesized via [`HANDS_TOOL`] when the active
    /// actionbar slot has no tool. Crude pickup nodes carry a
    /// `ToolRequirement` of `Hands` to mark themselves as
    /// E-pickup-only, no tool (including bare hands) can gather them
    /// by swinging. See [`crate::resource_nodes::ToolRequirement::allows`].
    /// Also the [`Default`], so a freshly-spawned peer swing action reads
    /// as "empty-handed punch" until the first real swing arrives.
    #[default]
    Hands,
    Axe,
    Pickaxe,
    /// Construction hammer. Never gathers and never damages; its swing
    /// repairs building blocks and its held-right-click wheel upgrades or
    /// demolishes them.
    Hammer,
}

impl ToolKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hands => "Bare hands",
            Self::Axe => "Hatchet",
            Self::Pickaxe => "Pickaxe",
            Self::Hammer => "Hammer",
        }
    }

    /// The swing/impact [`ItemModel`] archetype this tool animates and reads as
    /// on the wire. Iron and stone tools of a kind share an archetype (the mesh
    /// differs, the swing does not). The hammer chops like the hatchet (a
    /// deliberate-work repair tap); bare hands read as the short bag "punch".
    /// This is the single source of truth both the server (deriving the wire
    /// `ItemModel` for a gather-tool swing) and the client (its local swing
    /// timing and pose) share, so a tool's peer-visible identity can never
    /// disagree between the two sides.
    pub const fn swing_model(self) -> ItemModel {
        match self {
            Self::Hands => ItemModel::Bag,
            // The hammer shares the hatchet's cadence: repair taps should feel
            // like deliberate work, same weight class of swing.
            Self::Axe | Self::Hammer => ItemModel::Hatchet,
            Self::Pickaxe => ItemModel::Pickaxe,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolProfile {
    pub kind: ToolKind,
    pub tier: u8,
    pub gather_amount: u16,
    pub cooldown_ticks: u64,
    /// Impacts the tool survives before breaking. Only swings that
    /// connect with something (gather payout, player hit, structure
    /// hit) consume durability; whiffs are free. `None` means the tool
    /// never wears (bare hands).
    pub max_durability: Option<u32>,
    /// Raw per-swing PvP damage before armor. `0` means the tool can't
    /// damage players at all (bare hands); the combat path rejects the
    /// swing instead of landing a zero-damage hit.
    pub player_damage: u32,
}

/// Synthesized tool profile used when no actionbar item is held. The
/// server substitutes this in when the active stack carries no tool
/// definition so the gather pipeline always has a `ToolProfile` to read.
/// It's never accepted as a valid gather tool, crude nodes are E-pickup
/// only and the tool-required nodes reject Hands explicitly, but it
/// keeps the cooldown/payout math uniform across the gather path.
pub const HANDS_TOOL: ToolProfile = ToolProfile {
    kind: ToolKind::Hands,
    tier: 0,
    gather_amount: 1,
    cooldown_ticks: 10,
    max_durability: None,
    player_damage: 0,
};
