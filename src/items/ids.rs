//! Stable string identifiers for every registered item. These back
//! `ItemStack`, item definitions, and recipe outputs; keep them stable
//! because they travel in saves and on the wire.

pub const WOOD_ID: &str = "wood";
pub const STONE_ID: &str = "stone";
pub const COAL_ID: &str = "coal";
pub const IRON_ORE_ID: &str = "iron_ore";
pub const IRON_BAR_ID: &str = "iron_bar";
pub const SULFUR_ORE_ID: &str = "sulfur_ore";
/// Refined sulfur, smelted from sulfur ore in a furnace. One half of the
/// gunpowder recipe.
pub const SULFUR_ID: &str = "sulfur";
/// Coarse blasting powder, ground from coal and sulfur at a workbench. The
/// base charge behind the explosives.
pub const GUNPOWDER_ID: &str = "gunpowder";
pub const FIBER_ID: &str = "fiber";
/// Woven fiber cloth. The padding and wrapping behind the cloth armor set.
pub const CLOTH_ID: &str = "cloth";
/// Rolled linen strip. Held down to bind a wound: heals instantly on
/// completion, then trickles the rest in over the following seconds.
pub const BANDAGE_ID: &str = "bandage";
/// Raw sky-metal pried out of meteorite nodes: a natural iron-nickel alloy
/// fused into the cooled slag. Smelts into meteorite ingots in a furnace.
pub const METEORITE_ALLOY_ID: &str = "meteorite_alloy";
/// Refined bar smelted from meteorite alloy. The metal behind the workbench
/// tier-2 upgrade and (later) top-tier gear.
pub const METEORITE_INGOT_ID: &str = "meteorite_ingot";
/// Salvaged mechanisms (hinges, latches, springs) stripped from burnt-out
/// houses. Cannot be crafted; the "scrap" sink for the workbench tier-2
/// upgrade and later crossbow / iron-armor recipes.
pub const SALVAGED_FITTINGS_ID: &str = "salvaged_fittings";
pub const PLANT_TWINE_ID: &str = "plant_twine";
/// Refined wood. Raw `wood` worked into a clean structural billet at a
/// workbench, the handle stock for tier-2 tools and the building block for
/// later construction.
pub const HEWN_LOG_ID: &str = "hewn_log";
pub const BASIC_HATCHET_ID: &str = "wood_stone_hatchet";
pub const BASIC_PICKAXE_ID: &str = "wood_stone_pickaxe";
pub const IRON_HATCHET_ID: &str = "iron_hatchet";
pub const IRON_PICKAXE_ID: &str = "iron_pickaxe";
/// Hand-craftable harvesting sickle. Gathers nothing from resource nodes;
/// its swing sweeps plant fiber out of the instanced grass carpet, scaled by
/// the biome's grass density (see `crate::server::harvest`).
pub const IRON_SICKLE_ID: &str = "iron_sickle";
/// The three melee weapons. They carry a `WeaponProfile` (not a
/// `ToolProfile`), so they gather nothing and combat resolves them ahead of any
/// tool. Meshes and icons live at `assets/items/<id>/{model.glb,icon.png}`.
pub const WOODEN_CLUB_ID: &str = "wooden_club";
pub const STONE_SPEAR_ID: &str = "stone_spear";
pub const IRON_SWORD_ID: &str = "iron_sword";
/// The two ranged weapons and their shared ammunition. The bow and
/// crossbow carry a `RangedProfile` (not a `ToolProfile` or `WeaponProfile`), so
/// they gather nothing and fire a server-simulated projectile instead of
/// swinging. The arrow is plain stackable ammo (no profile), consumed one per
/// shot and ~50% recoverable from world hits. Meshes and icons live at
/// `assets/items/<id>/{model.glb,icon.png}`.
pub const WOODEN_BOW_ID: &str = "wooden_bow";
pub const CROSSBOW_ID: &str = "crossbow";
pub const ARROW_ID: &str = "arrow";
/// The three blackpowder explosives. Each carries an `ExplosiveProfile` (not
/// a `ToolProfile`/`WeaponProfile`), so it gathers nothing and does no melee
/// damage. The bomb is thrown; the keg and satchel are placed. Meshes and
/// icons live at `assets/items/<id>/{model.glb,icon.png}`.
pub const POWDER_BOMB_ID: &str = "powder_bomb";
pub const POWDER_KEG_ID: &str = "powder_keg";
pub const SATCHEL_CHARGE_ID: &str = "satchel_charge";
pub const WORKBENCH_T1_ID: &str = "workbench_t1";
pub const CRUDE_FURNACE_ID: &str = "crude_furnace";
/// Holdable blueprint that drives the building system: hold right click to
/// pick a piece, left click to place its ghost.
pub const BUILDING_PLAN_ID: &str = "building_plan";
/// Construction hammer: left click repairs building blocks, hold right
/// click for the upgrade / demolish wheel.
pub const HAMMER_ID: &str = "hammer";
/// Code-locked door deployable that mounts only in doorway openings.
pub const HEWN_LOG_DOOR_ID: &str = "hewn_log_door";
/// Iron door variant: same code lock + doorway mount as the hewn log door,
/// but tool-immune (only future explosives breach it) and double the HP.
pub const IRON_DOOR_ID: &str = "iron_door";
/// Codeless window shutter: mounts only in window-wall openings; toggled by
/// the owner or anyone authorized on the covering Tool Cupboard.
pub const WOOD_SHUTTER_ID: &str = "wood_shutter";
/// Respawn-anchor deployable crafted from plant fiber.
pub const SLEEPING_BAG_ID: &str = "sleeping_bag";
/// Placeable item containers; small is hand-craftable, large needs a
/// workbench.
pub const STORAGE_BOX_SMALL_ID: &str = "storage_box_small";
pub const STORAGE_BOX_LARGE_ID: &str = "storage_box_large";
/// Placeable light source crafted from wood + coal. Burns ~8 hours, then
/// goes dark. Mounts on the ground or the side of a wall.
pub const TORCH_ID: &str = "torch";

/// The base-claim Tool Cupboard deployable.
pub const TOOL_CUPBOARD_ID: &str = "tool_cupboard";

/// The ruin loot cache: a small charred-wood, iron-banded chest spawned
/// inside the burnt-out houses at world generation. Not craftable and not
/// placeable by players (`equipable: false`); it exists only as the
/// deployable a ruin site spawns. Anyone can open it, and it refills its
/// loot on a timer. It is the exclusive source of `salvaged_fittings`.
pub const RUIN_CACHE_ID: &str = "ruin_cache";

/// Padded (cloth) armor set, one id per worn slot. The starter armor set:
/// cheap, hand-crafted, worn on the paperdoll.
pub const PADDED_HOOD_ID: &str = "padded_hood";
pub const PADDED_TUNIC_ID: &str = "padded_tunic";
pub const PADDED_LEGGINGS_ID: &str = "padded_leggings";
pub const PADDED_WRAPS_ID: &str = "padded_wraps";

/// Lamellar (hewn-wood slats over cloth) armor set, one id per worn slot. The
/// mid-tier set: crafted at a workbench, twice the protection of padded. Meshes
/// and icons live at `assets/items/<id>/{model.glb,icon.png}`.
pub const LAMELLAR_HELM_ID: &str = "lamellar_helm";
pub const LAMELLAR_VEST_ID: &str = "lamellar_vest";
pub const LAMELLAR_GREAVES_ID: &str = "lamellar_greaves";
pub const LAMELLAR_BOOTS_ID: &str = "lamellar_boots";

/// Iron (plate over padding) armor set, one id per worn slot. The top set of
/// the tree: forged at a tier-2 workbench with looted salvaged fittings.
/// Meshes and icons live at `assets/items/<id>/{model.glb,icon.png}`.
pub const IRON_HELM_ID: &str = "iron_helm";
pub const IRON_CUIRASS_ID: &str = "iron_cuirass";
pub const IRON_GREAVES_ID: &str = "iron_greaves";
pub const IRON_BOOTS_ID: &str = "iron_boots";
