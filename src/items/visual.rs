//! First-person visual selectors for a held item: the swing archetype
//! ([`ItemModel`]), the in-hand mesh ([`HeldMesh`]), and the icon tint
//! ([`ItemTint`]). All three are decoupled so a tool's look, animation, and
//! color can vary independently.

/// First-person *animation archetype* for a held item. Drives the swing
/// pose and the tool-swap lift cadence, not the mesh. Iron and stone tools
/// of the same kind share an archetype (an iron hatchet swings exactly like
/// a stone one); only their [`HeldMesh`] differs. Keeping this coarse means
/// adding a new tool material never touches the pose curves.
///
/// Serde-derived because this is the swing/impact *identity* carried on the
/// wire: the cosmetic [`crate::server::PlayerAction`] replicates it (a peer
/// animates a swing off it), and the one-shot `SwingStart`/`PlayerImpact`
/// messages carry it (so a peer's audio, VFX, and camera reaction key on the
/// weapon that landed the hit, not on a gather-tool stand-in). A 1-byte enum,
/// never an item-id string. `Default` is [`ItemModel::Bag`], the empty-hand /
/// non-combat archetype the wire falls back to for bare hands and the hammer.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum ItemModel {
    #[default]
    Bag,
    Hatchet,
    Pickaxe,
    /// Deployable items render as the bag silhouette in the held-item
    /// slot, the actual structure mesh is what gets placed in the world.
    Deployable,
    /// Wooden club: a short, quick chop.
    Club,
    /// Stone spear: a forward thrust (lunge along the aim axis), not an arc.
    Spear,
    /// Iron sword: a wide horizontal arc.
    Sword,
    /// Wooden bow: a hold-to-draw ranged pose (no swing arc). Drives the animated
    /// draw viewmodel: the limbs flex and the string pulls into a V off the draw
    /// fraction, with a forward-flick loose (see `held::held_piece_local_transform`).
    Bow,
    /// Crossbow: a shoulder-braced ranged pose with a heavy reload cycle rather
    /// than a swing. Sits cocked and level at ready, snaps the string forward with
    /// a recoil kick on fire, and dips down to be cranked back up on reload.
    Crossbow,
    /// Thrown powder bomb: a short wind-up-and-release lob (not a swing arc and
    /// not a ranged draw). Drives its own light toss pose off a throw clock so a
    /// bomb reads as a committed overhand toss with a real release beat.
    /// APPEND-ONLY (this rides the wire on `PlayerHeldItem`/`PlayerAction`); a new
    /// variant goes at the end and never reorders.
    ThrownBomb,
    /// Bandage: a hold-to-use consumable. Neither swings nor fires. The roll is
    /// raised across the body and the loose tail visibly unrolls as the use charge
    /// ramps (see `held::held_piece_local_transform`), then a quick cinch on
    /// completion. Its charge fraction, like the bow's draw, comes from the server.
    Bandage,
    /// Sickle: a low horizontal reaping slash. Unlike the sword's shoulder-high
    /// whip, the whole cut stays down at grass height: draw out level to the
    /// right, sweep flat across the frame, exit left. Appended LAST (this
    /// archetype rides the wire on `PlayerAction`/`SwingStart`/`PlayerImpact`).
    Sickle,
}

impl ItemModel {
    /// Every [`ItemModel`] variant, so the completeness tests (every model
    /// resolves an impact sound and a camera-kick profile) can assert each is
    /// covered. Adding a variant is a compile error here until it is listed.
    pub const ALL: &'static [ItemModel] = &[
        ItemModel::Bag,
        ItemModel::Hatchet,
        ItemModel::Pickaxe,
        ItemModel::Deployable,
        ItemModel::Club,
        ItemModel::Spear,
        ItemModel::Sword,
        ItemModel::Bow,
        ItemModel::Crossbow,
        ItemModel::ThrownBomb,
        ItemModel::Bandage,
        ItemModel::Sickle,
    ];
}

/// Which first-person *mesh* the registry tells the renderer to put in the
/// player's hand. Decoupled from [`ItemModel`] so a tool's look (stone vs
/// iron head) is independent of how it animates. Raw materials and
/// deployables-in-hand fall back to the generic bag silhouette. Adding a
/// new tool material is a new variant here plus one mesh handle, no pose or
/// gameplay code changes.
///
/// Serde-derived because the peer-visible [`crate::server::PlayerHeldItem`]
/// component replicates this 1-byte selector (not the `Arc<str>` item id) so
/// remote players can render what another player is holding without shipping a
/// string every diff or re-resolving the registry on the peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HeldMesh {
    Bag,
    StoneHatchet,
    IronHatchet,
    StonePickaxe,
    IronPickaxe,
    /// Construction hammer (a wooden mallet): wood body + iron band hoops.
    Hammer,
    /// Rolled-up building plan scroll.
    BuildingPlan,
    /// Wooden club: an all-wood one-hander (haft + knobbed head both Wood).
    WoodenClub,
    /// Stone spear: a long wooden haft (Wood) tipped with a knapped stone point
    /// (Stone).
    StoneSpear,
    /// Iron sword: a wood-wrapped grip (Wood) and a forged iron blade (Iron).
    IronSword,
    /// Wooden bow: an ANIMATABLE five-primitive glb (grip + two limbs on the Wood
    /// family, two string legs on the Cord family). The limbs flex and the string
    /// pulls into a V as the draw ramps (see `held::held_piece_local_transform`).
    WoodenBow,
    /// Crossbow: a wood stock (Wood), iron fittings (Iron), and an animatable
    /// string (Cord) that slides forward on release / back on the reload crank.
    Crossbow,
    /// Arrow: a wood shaft (Wood) tipped with a knapped stone head (Stone).
    Arrow,
    // explosives. APPEND-ONLY: this enum is serialised on the wire in
    // `PlayerHeldItem`, so new variants go at the end and never reorder.
    /// Powder bomb: a cloth-wrapped ball (Cloth) with an iron fuse cap (Iron).
    PowderBomb,
    /// Powder keg: a staved wooden barrel (Wood) bound by iron hoops (Iron).
    PowderKeg,
    /// Satchel charge: a cloth-and-leather pack (Cloth) with a leather strap
    /// (Leather).
    SatchelCharge,
    /// Bandage: an ANIMATABLE two-primitive glb, both on the Cloth family. The
    /// roll is static; the loose tail is its own primitive so it can visibly
    /// UNROLL out of the roll as the use charge ramps (see
    /// `held::held_piece_local_transform`).
    Bandage,
    /// Iron sickle: the grass-harvesting tool. A wood haft (prim 0) under a
    /// broad forged-iron crescent (prim 1), the classic haft+head split.
    /// Appended LAST (this selector rides the wire in `PlayerHeldItem`).
    Sickle,
}

impl HeldMesh {
    /// Every [`HeldMesh`] variant, so the visual-registry completeness test can
    /// assert each has a table row. Adding a variant without extending
    /// [`HeldMesh::visual`] then fails CI instead of rendering nothing.
    pub const ALL: &'static [HeldMesh] = &[
        HeldMesh::Bag,
        HeldMesh::StoneHatchet,
        HeldMesh::IronHatchet,
        HeldMesh::StonePickaxe,
        HeldMesh::IronPickaxe,
        HeldMesh::Hammer,
        HeldMesh::BuildingPlan,
        HeldMesh::WoodenClub,
        HeldMesh::StoneSpear,
        HeldMesh::IronSword,
        HeldMesh::WoodenBow,
        HeldMesh::Crossbow,
        HeldMesh::Arrow,
        HeldMesh::PowderBomb,
        HeldMesh::PowderKeg,
        HeldMesh::SatchelCharge,
        HeldMesh::Bandage,
        HeldMesh::Sickle,
    ];

    /// The declarative in-hand visual for this mesh: one [`HeldLayerSpec`] per
    /// overlaid layer (most items are a single layer; the authored tool glbs are
    /// two, a haft body plus a worked head). This is the single source of truth
    /// the renderer folds into concrete mesh + material handles, replacing the
    /// former per-item handle fields on `ItemVisualAssets` and the match arm in
    /// `held_item_layers`. Adding a held item is one row here (plus its glb),
    /// not code across three files.
    pub const fn visual(self) -> HeldMeshVisual {
        // The four tools share three material families: the haft plus its twine
        // ride Wood, the worked head rides Stone (knapped) or Iron (forged). The
        // hammer's bands ride Iron; the building-plan paper rides Parchment and
        // its ties reuse Wood. Per-item colour is baked into each glb's COLOR_0,
        // so the family is only which shared material a layer binds.
        match self {
            // The bag silhouette covers deployables-in-hand: the generic
            // carried bundle (a tied burlap sack, image-to-3D like the tools
            // but WITHOUT a grip socket; it keeps the legacy silhouette
            // placement).
            HeldMesh::Bag => {
                HeldMeshVisual::baked_tool("items/generic_held/model.glb", "generic_held")
            }
            // The five gathering tools are image-to-3D rebuilds (art/held/):
            // ONE primitive with real UVs and a per-item baked albedo (the
            // `Baked` family), plus a `socket_grip` node the engine reads for
            // hand placement instead of per-item Rust constants.
            HeldMesh::StoneHatchet => HeldMeshVisual::baked_tool(
                "items/wood_stone_hatchet/model.glb",
                "wood_stone_hatchet",
            ),
            HeldMesh::StonePickaxe => HeldMeshVisual::baked_tool(
                "items/wood_stone_pickaxe/model.glb",
                "wood_stone_pickaxe",
            ),
            HeldMesh::Sickle => {
                HeldMeshVisual::baked_tool("items/iron_sickle/model.glb", "iron_sickle")
            }
            HeldMesh::IronHatchet => {
                HeldMeshVisual::baked_tool("items/iron_hatchet/model.glb", "iron_hatchet")
            }
            HeldMesh::IronPickaxe => {
                HeldMeshVisual::baked_tool("items/iron_pickaxe/model.glb", "iron_pickaxe")
            }
            // The hammer, the building-plan scroll, the four melee weapons, the
            // arrow, and the three explosives are batch-2 image-to-3D rebuilds:
            // one primitive, real UVs, per-item baked albedo. Unlike the five
            // gathering tools they carry NO grip socket; their heavily tuned
            // per-item carry poses (mallet pull-in, upright sword guard, couched
            // spear, silhouette bundles) live in `held.rs` keyed on the same
            // local frames the old authored glbs used, so each rebuild is
            // fitted into its predecessor's frame at build time (art/held/).
            HeldMesh::Hammer => HeldMeshVisual::baked_tool("items/hammer/model.glb", "hammer"),
            HeldMesh::BuildingPlan => {
                HeldMeshVisual::baked_tool("items/building_plan/model.glb", "building_plan")
            }
            HeldMesh::WoodenClub => {
                HeldMeshVisual::baked_tool("items/wooden_club/model.glb", "wooden_club")
            }
            HeldMesh::StoneSpear => {
                HeldMeshVisual::baked_tool("items/stone_spear/model.glb", "stone_spear")
            }
            HeldMesh::IronSword => {
                HeldMeshVisual::baked_tool("items/iron_sword/model.glb", "iron_sword")
            }
            // The bow and crossbow are ANIMATABLE multi-primitive glbs (their
            // limbs / string bend off the draw): the bow is five pieces (grip, two
            // limbs, two Cord string legs), the crossbow three (stock, iron, Cord
            // string). The arrow stays a plain two-primitive glb (wood shaft +
            // stone head).
            // The wooden bow is an ANIMATABLE five-primitive glb: a static grip,
            // two flexing limbs, and two string legs. Each layer is tagged with its
            // rig slot so the per-piece animator (see `held::held_piece_local_transform`)
            // can flex the limbs and pull the string legs off the draw fraction; the
            // grip stays static. The three wood pieces ride Wood; the two string legs
            // ride the pale Cord family. Primitive order matches the glb:
            // 0 grip, 1 limb_upper, 2 limb_lower, 3 string_upper, 4 string_lower.
            HeldMesh::WoodenBow => HeldMeshVisual::bow("items/wooden_bow/model.glb"),
            // The crossbow is a THREE-primitive glb: a static wood stock, static iron
            // fittings, and an animatable string. The string slot slides forward on
            // release / back on the reload crank off the cock value.
            HeldMesh::Crossbow => HeldMeshVisual::crossbow("items/crossbow/model.glb"),
            HeldMesh::Arrow => HeldMeshVisual::baked_tool("items/arrow/model.glb", "arrow"),
            // The three explosive glbs double as the placed-charge / thrown
            // projectile world models, so they stay authored at WORLD scale
            // (see `viewmodel_scale`) and keep their old origins (the bomb's
            // is the ball's bottom, the placed charges sit on their base).
            HeldMesh::PowderBomb => {
                HeldMeshVisual::baked_tool("items/powder_bomb/model.glb", "powder_bomb")
            }
            HeldMesh::PowderKeg => {
                HeldMeshVisual::baked_tool("items/powder_keg/model.glb", "powder_keg")
            }
            HeldMesh::SatchelCharge => {
                HeldMeshVisual::baked_tool("items/satchel_charge/model.glb", "satchel_charge")
            }
            // The bandage is a two-primitive glb, BOTH on the Cloth family (roll
            // and tail are the same linen; only the COLOR_0 differs). Unlike the
            // other two-prim items it is not static: primitive 1 is tagged
            // `BandageTail` so the per-piece animator can unroll it off the use
            // charge. Primitive order matches the glb: 0 roll, 1 tail.
            HeldMesh::Bandage => HeldMeshVisual::bandage("items/bandage/model.glb"),
        }
    }

    /// First-person viewmodel scale for this mesh. `1.0` for the hand-authored
    /// tools/weapons; smaller for the explosives, whose glbs are authored at
    /// WORLD scale (they double as the placed-charge / projectile models): a
    /// 0.55 m barrel carried at arm's length would fill half the frame, so the
    /// viewmodel shrinks it into a carried-prop read tucked at the
    /// bottom-right. Applied only by the first-person renderer; the
    /// third-person rig and the projectile/placed visuals use the same meshes
    /// unscaled.
    pub const fn viewmodel_scale(self) -> f32 {
        match self {
            HeldMesh::PowderKeg => 0.45,
            HeldMesh::SatchelCharge => 0.60,
            HeldMesh::PowderBomb => 0.70,
            // The bandage glb is authored at true world scale (a ~0.20 m roll),
            // which at 1.0 fills a third of the frame in first person. Shrink it to
            // a hand-prop read.
            HeldMesh::Bandage => 0.42,
            _ => 1.0,
        }
    }

    /// Whether this mesh's glb carries a `socket_grip` node the engine derives
    /// hand placement from (the five gathering-tool rebuilds, ART-PIPELINE
    /// Phase 0 contract). The batch-2 rebuilds deliberately ship WITHOUT a
    /// socket: their per-item carry poses (mallet pull-in, upright sword
    /// guard, couched spear, silhouette bundles) are tuned in `held.rs`
    /// against each mesh's authored frame, and the socket path would bypass
    /// all of that tuning. Gating the socket load here (not on the `Baked`
    /// family) keeps startup free of pointless Gltf scans + missing-socket
    /// warnings for the socketless rebuilds.
    pub const fn uses_grip_socket(self) -> bool {
        matches!(
            self,
            HeldMesh::StoneHatchet
                | HeldMesh::IronHatchet
                | HeldMesh::StonePickaxe
                | HeldMesh::IronPickaxe
                | HeldMesh::Sickle
        )
    }

    /// The first-person *grip archetype* this mesh is carried by. The renderer
    /// turns the archetype into a concrete hand transform (grip orientation,
    /// where down the haft the hand grips, and any carry offset); a `HeldGrip`
    /// is a small bounded set of carry poses, not a per-item value, so a new
    /// same-shaped item (another long-hafted weapon, say) reuses an existing
    /// archetype and never adds a grip branch in the renderer. This is the grip
    /// analogue of [`ItemModel`] for swing poses: keeping it here means a new
    /// [`HeldMesh`] is a data row, not a match arm in `held.rs`.
    pub const fn grip(self) -> HeldGrip {
        match self {
            // Bag silhouette, the rolled scroll, and the nocked arrow have no
            // handle; they sit upright with no grip offset. The arrow rides here
            // as a placeholder until P3b gives it a real draw-nock pose. The
            // thrown bomb and the two carried charges (keg, satchel) are held
            // bundles with no haft, so they carry the same way as the bag.
            HeldMesh::Bag
            | HeldMesh::BuildingPlan
            | HeldMesh::Arrow
            | HeldMesh::PowderBomb
            | HeldMesh::PowderKeg
            | HeldMesh::SatchelCharge
            | HeldMesh::Bandage => HeldGrip::Silhouette,
            // The bladed/pointed long tools (and dedicated weapons that reuse
            // this look) are gripped low on a haft carried out front. The sword
            // sits here for its blade-forward silhouette, the spear for its long
            // point-forward haft, and the crossbow for its held-out-front ranged
            // carry (its reload animation rides the per-piece transform + the
            // ranged pose, not this carry grip).
            HeldMesh::StoneHatchet
            | HeldMesh::IronHatchet
            | HeldMesh::StonePickaxe
            | HeldMesh::IronPickaxe
            | HeldMesh::IronSword
            | HeldMesh::Sickle
            | HeldMesh::Crossbow => HeldGrip::LongHafted,
            // The bow carries upright with no quarter-turn yaw; its draw rides the
            // per-piece transform + ranged pose on top of this rest carry.
            HeldMesh::WoodenBow => HeldGrip::Bow,
            // The spear carries couched down the aim (point forward), matching the
            // first-person viewmodel; the thrust rides the arm extension on top.
            HeldMesh::StoneSpear => HeldGrip::Spear,
            // Short one-handers held close in: the construction mallet and the
            // blunt wooden club.
            HeldMesh::Hammer | HeldMesh::WoodenClub => HeldGrip::Mallet,
        }
    }

    /// Mesh-local grip point for meshes WITHOUT an authored `socket_grip`
    /// node: the exact point on the handle the fist closes around. This is the
    /// single source of truth shared by the first-person seat and the
    /// third-person hand placement, so both views grip the identical spot on
    /// the item (owner rule: the grip is accurate and the same everywhere;
    /// per-view composition is tuned via the arm/carry offsets, never by
    /// moving the grip). Socket glbs carry this point inside the asset
    /// instead; meshes returning `None` fall back to their archetype's
    /// generic grip height.
    pub const fn grip_point(self) -> Option<[f32; 3]> {
        match self {
            // The sword's wrapped handle spans local y [-0.50, -0.28] under
            // the guard; the fist closes low on the wrap so the pommel peeks
            // just below the hand. (The old third-person LongHafted fallback
            // put the fist at y = -0.16, visibly ON the blade.)
            HeldMesh::IronSword => Some([0.0, -0.44, 0.0]),
            // Mid-shaft on the spear (point at ~+0.65, butt at ~-0.5): the
            // couched thrusting grip, with the butt riding well behind the
            // hand. Matches the first-person mid-shaft seat; the third
            // person used to grip near the butt end.
            HeldMesh::StoneSpear => Some([0.0, 0.22, 0.0]),
            _ => None,
        }
    }
}

/// First-person carry archetype for a held mesh: how the hand grips it and how
/// it sits relative to the camera. A small, bounded set of poses (not a per-item
/// value) so the renderer maps each to a concrete transform once and a new
/// same-shaped [`HeldMesh`] reuses an existing archetype. The grip analogue of
/// [`ItemModel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldGrip {
    /// No handle: the bag silhouette and the building-plan scroll. Sits upright
    /// at the hand with no grip offset.
    Silhouette,
    /// A long haft carried out front, gripped low toward the butt: the hatchets,
    /// the pickaxes, and dedicated long melee weapons that reuse that look.
    LongHafted,
    /// A short one-handed mallet held close to the body: the construction hammer.
    Mallet,
    /// A bow held upright in the fist: the stave vertical, the flat of the bow
    /// edge-on to the archer, and the down-range (target) side pointing forward.
    /// Unlike a hafted tool it takes no quarter-turn yaw (that yaw is what spun
    /// the bow the wrong way in third-person); the draw animation rides the
    /// per-piece transform + ranged pose on top of this rest carry.
    Bow,
    /// A spear held COUCHED down the aim: the shaft laid horizontal with the
    /// point forward and the butt back by the hip, matching the first-person
    /// viewmodel's couched carry (the hafted-tool grip left it standing upright,
    /// which read nothing like the thrust weapon it is). The forward thrust rides
    /// the arm extension on top of this stance.
    Spear,
}

/// Which shared material family a held-item layer binds. Resolves to the
/// existing shared handles on `ItemVisualAssets` in both a world-lit
/// (`ToonMaterial`) and a first-person camera-relative (`ToonViewmodelMaterial`)
/// variant, plus a `Standard` arm for the layers that still use a
/// `StandardMaterial` today (the bag). Keeping the family as data means the
/// in-flight tools-PBR rework can flip a family to `StandardMaterial` in one
/// place without touching the per-item table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldMeshMaterial {
    /// Wooden pieces of the authored animatable glbs (bow stave + nocked
    /// arrow, crossbow stock + bolt).
    Wood,
    /// Forged iron: the crossbow's fittings.
    Iron,
    /// Woven cloth: the bandage's roll and tail.
    Cloth,
    /// Pale tan bowstring / crossbow-string cord. A slim neutral cord tile whose
    /// COLOR_0 carries the pale-tan string colour, the same `detail * COLOR_0` cel
    /// path the other families use.
    Cord,
    /// A per-item BAKED albedo (image-to-3D rebuilds, `art/held/`): the glb ships
    /// white COLOR_0 + real UVs and the texture at `textures/held/<id>.png`
    /// carries the whole painted surface. The str is the item id the renderer
    /// keys the per-item material pair on.
    Baked(&'static str),
}

/// Where a held-item layer's mesh comes from. Every held item is an authored
/// glb primitive now (the procedural bag cuboid retired to the image-to-3D
/// bundle rebuild); the enum stays so a future non-glb source is a variant,
/// not a rework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldLayerMeshSource {
    /// Primitive `primitive` of the (mesh 0 of the) glb at `glb`, an
    /// `embedded_asset_path`-relative path (no `embedded://` prefix).
    GlbPrimitive { glb: &'static str, primitive: usize },
}

/// Which animatable RIG PIECE a held-item layer is, so the per-piece animator in
/// `held::held_piece_local_transform` knows whether (and how) to give the layer a
/// local transform on top of the whole-item swing transform. Every existing
/// single-transform item is [`HeldPieceSlot::Static`] (identity local transform),
/// so the melee / tool path is byte-unchanged; only the bow limbs / string and the
/// crossbow string carry a driven slot. Kept here as data (not a match in `held.rs`)
/// so the mesh table stays the single source of the mesh -> rig-slot mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldPieceSlot {
    /// A static piece: the whole-item transform is the entire transform (identity
    /// piece-local). Every melee / tool layer, plus the bow grip and the crossbow
    /// stock / iron, is Static.
    Static,
    /// The bow's upper limb: flexes toward the target about its authored pivot as
    /// the draw fraction ramps.
    BowLimbUpper,
    /// The bow's lower limb: mirror of the upper limb.
    BowLimbLower,
    /// The bow's upper string leg: pinned at the upper limb tip, its free (nock)
    /// end tracks the drawn nock point.
    BowStringUpper,
    /// The bow's lower string leg: mirror of the upper string leg.
    BowStringLower,
    /// The bow's nocked arrow: rides the string nock as the draw pulls back, so
    /// a ready arrow always shows and its tip is the full-draw aim reference.
    /// Collapses right after loose while the shot arrow flies.
    BowArrow,
    /// The crossbow's string: the nut slides forward on release / back on the
    /// reload crank, and each leg tracks it from its limb tip.
    CrossbowString,
    /// The crossbow's loaded bolt: rides the string nut while cocked, collapses
    /// on fire (the real projectile is flying) and seats back in as the reload
    /// crank finishes.
    CrossbowBolt,
    /// The bandage's loose tail: rooted at the roll's bottom tangent and scaled
    /// out along its length as the use charge ramps, so the strip visibly unrolls
    /// in hand. Authored at FULL extension in the glb (see
    /// art/consumables/build_consumables.py), so the rest pose scales it back in.
    BandageTail,
}

/// One overlaid layer of a held item: its mesh source, material family, and rig
/// slot (which drives any per-piece local transform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldLayerSpec {
    pub mesh: HeldLayerMeshSource,
    pub material: HeldMeshMaterial,
    pub slot: HeldPieceSlot,
}

/// The full in-hand visual for a [`HeldMesh`]: up to six overlaid layers sharing
/// one whole-item swing transform, each optionally driven by its own per-piece
/// local transform (see [`HeldPieceSlot`]). Fixed capacity (no allocation); six
/// is the animatable bow's limb + string + nocked-arrow count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldMeshVisual {
    layers: [Option<HeldLayerSpec>; 6],
}

impl HeldMeshVisual {
    const fn single(layer: HeldLayerSpec) -> Self {
        Self {
            layers: [Some(layer), None, None, None, None, None],
        }
    }

    /// A single-primitive image-to-3D glb (`art/held/`): the whole item is one
    /// mesh whose painted surface lives in the per-item baked albedo, so one
    /// layer on the [`HeldMeshMaterial::Baked`] family. The five gathering
    /// tools additionally carry the `socket_grip` node the engine derives hand
    /// placement from (see [`HeldMesh::uses_grip_socket`]); every other rebuild
    /// keeps its legacy tuned carry.
    const fn baked_tool(glb: &'static str, item_id: &'static str) -> Self {
        Self::single(Self::glb_layer(
            glb,
            0,
            HeldMeshMaterial::Baked(item_id),
            HeldPieceSlot::Static,
        ))
    }

    /// The animatable bandage: two AUTHORED primitives, 0 the roll (static)
    /// and 1 the loose tail (it scales out of the roll as the use charge
    /// ramps), both on the Cloth family. Still the old authored glb: the
    /// batch-2 rebuild's roll fit kept reading as a flat wedge in hand, so
    /// its rebuild is parked with the crossbow's (the new icon ships
    /// regardless).
    const fn bandage(glb: &'static str) -> Self {
        Self {
            layers: [
                Some(Self::glb_layer(
                    glb,
                    0,
                    HeldMeshMaterial::Cloth,
                    HeldPieceSlot::Static,
                )),
                Some(Self::glb_layer(
                    glb,
                    1,
                    HeldMeshMaterial::Cloth,
                    HeldPieceSlot::BandageTail,
                )),
                None,
                None,
                None,
                None,
            ],
        }
    }

    /// The animatable wooden bow: six primitives (grip, upper limb, lower limb,
    /// upper string leg, lower string leg, nocked arrow) in the glb's primitive
    /// order. The stave pieces are ONE image-to-3D rebuild fitted to the
    /// authored anchors and bisected at the limb pivots
    /// (art/held/build_animatable.py), riding the per-item baked albedo; the
    /// two rebuilt string legs ride Cord; the reused nocked arrow keeps its
    /// authored COLOR_0 on Wood. Each carries its rig slot so the per-piece
    /// animator can flex the limbs, pull the string, and slide the nocked
    /// arrow back with it.
    const fn bow(glb: &'static str) -> Self {
        Self {
            layers: [
                Some(Self::glb_layer(
                    glb,
                    0,
                    HeldMeshMaterial::Baked("wooden_bow"),
                    HeldPieceSlot::Static,
                )),
                Some(Self::glb_layer(
                    glb,
                    1,
                    HeldMeshMaterial::Baked("wooden_bow"),
                    HeldPieceSlot::BowLimbUpper,
                )),
                Some(Self::glb_layer(
                    glb,
                    2,
                    HeldMeshMaterial::Baked("wooden_bow"),
                    HeldPieceSlot::BowLimbLower,
                )),
                Some(Self::glb_layer(
                    glb,
                    3,
                    HeldMeshMaterial::Cord,
                    HeldPieceSlot::BowStringUpper,
                )),
                Some(Self::glb_layer(
                    glb,
                    4,
                    HeldMeshMaterial::Cord,
                    HeldPieceSlot::BowStringLower,
                )),
                Some(Self::glb_layer(
                    glb,
                    5,
                    HeldMeshMaterial::Wood,
                    HeldPieceSlot::BowArrow,
                )),
            ],
        }
    }

    /// The animatable crossbow: four AUTHORED primitives (wood stock, iron
    /// fittings, string, loaded bolt). Still the old authored glb: the
    /// batch-2 image-to-3D body reconstructed with a tall rifle stock that
    /// blocked the owner-tuned ADS sight picture, so its rebuild is parked
    /// until a better reference or a re-tuned aim pose exists (its new icon
    /// ships regardless). The stock and iron are static; the string slides
    /// off the cock value and the bolt rides it, showing only while cocked.
    const fn crossbow(glb: &'static str) -> Self {
        Self {
            layers: [
                Some(Self::glb_layer(
                    glb,
                    0,
                    HeldMeshMaterial::Wood,
                    HeldPieceSlot::Static,
                )),
                Some(Self::glb_layer(
                    glb,
                    1,
                    HeldMeshMaterial::Iron,
                    HeldPieceSlot::Static,
                )),
                Some(Self::glb_layer(
                    glb,
                    2,
                    HeldMeshMaterial::Cord,
                    HeldPieceSlot::CrossbowString,
                )),
                Some(Self::glb_layer(
                    glb,
                    3,
                    HeldMeshMaterial::Wood,
                    HeldPieceSlot::CrossbowBolt,
                )),
                None,
                None,
            ],
        }
    }

    const fn glb_layer(
        glb: &'static str,
        primitive: usize,
        material: HeldMeshMaterial,
        slot: HeldPieceSlot,
    ) -> HeldLayerSpec {
        HeldLayerSpec {
            mesh: HeldLayerMeshSource::GlbPrimitive { glb, primitive },
            material,
            slot,
        }
    }

    /// The layers of this visual, in draw order.
    pub fn layers(&self) -> impl Iterator<Item = HeldLayerSpec> + '_ {
        self.layers.iter().flatten().copied()
    }
}

/// Which armor *mesh* the renderer attaches to a remote player's rig for a worn
/// piece. Decoupled from the item id the same way [`HeldMesh`] is, so a set's
/// look is a data selector rather than a string.
///
/// Serde-derived because the peer-visible [`crate::server::PlayerEquipmentVisual`]
/// component replicates these 1-byte selectors (one per worn slot) so remote
/// players can render another player's armor without shipping item-id strings or
/// re-resolving the registry per diff. No rig rendering consumes it yet (that is
/// Phase 4); this package only lands the wire selector and derives it
/// server-side. Adding a set is a new variant here plus its [`ArmorProfile`](crate::items::ArmorProfile)
/// row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArmorMesh {
    /// Quilted cloth hood (padded set, head slot).
    PaddedHood,
    /// Quilted cloth tunic (padded set, chest slot).
    PaddedTunic,
    /// Quilted cloth leggings (padded set, legs slot).
    PaddedLeggings,
    /// Cloth foot wraps (padded set, feet slot).
    PaddedWraps,
    // lamellar set (hewn-wood slats over cloth). APPEND-ONLY: this
    // enum is serialised on the wire in `PlayerEquipmentVisual`, so new variants
    // go at the end and never reorder.
    /// Slatted wood helm (lamellar set, head slot).
    LamellarHelm,
    /// Slatted wood vest with a shoulder cap (lamellar set, chest slot).
    LamellarVest,
    /// Slatted wood greaves (lamellar set, legs slot).
    LamellarGreaves,
    /// Slatted wood boots (lamellar set, feet slot).
    LamellarBoots,
    // iron set (plate over padding). APPEND-ONLY, same as above.
    /// Iron plate helm (iron set, head slot).
    IronHelm,
    /// Iron cuirass with a pauldron cap (iron set, chest slot).
    IronCuirass,
    /// Iron plate greaves (iron set, legs slot).
    IronGreaves,
    /// Iron plate boots (iron set, feet slot).
    IronBoots,
}

impl ArmorMesh {
    /// Every [`ArmorMesh`] variant, so a completeness test can assert each has a
    /// registered piece behind it. Adding a variant without an [`ArmorProfile`](crate::items::ArmorProfile)
    /// row then fails a test rather than shipping an unreachable mesh selector.
    pub const ALL: &'static [ArmorMesh] = &[
        ArmorMesh::PaddedHood,
        ArmorMesh::PaddedTunic,
        ArmorMesh::PaddedLeggings,
        ArmorMesh::PaddedWraps,
        ArmorMesh::LamellarHelm,
        ArmorMesh::LamellarVest,
        ArmorMesh::LamellarGreaves,
        ArmorMesh::LamellarBoots,
        ArmorMesh::IronHelm,
        ArmorMesh::IronCuirass,
        ArmorMesh::IronGreaves,
        ArmorMesh::IronBoots,
    ];

    /// The declarative rig visual for this worn piece: its glb plus one
    /// [`ArmorLayerSpec`] per attached primitive. This is the armor analogue of
    /// [`HeldMesh::visual`] and the single source of truth the client folds into
    /// ready `(Handle<Mesh>, material)` layers with per-prim attachment joints
    /// (see `app::systems::armor`). Adding an armor set is one row here plus its
    /// glbs, no rig-attachment code.
    ///
    /// THE ART CONTRACT (P4a): every glb's primitive 0 is the `<id>_shell`,
    /// authored pivot-local for identity attach at its joint; ONLY the three
    /// chest pieces carry a primitive 1 `<id>_aux` (the shoulder/pauldron cap),
    /// authored symmetric in X so identity-attaching it at both the left and the
    /// right shoulder is correct with no mirroring transform. So a chest piece
    /// yields three attachments (torso shell on the body, an aux cap on each
    /// upper arm); every other piece yields its shell mirrored across the L/R
    /// joints (helm on the body head region, greaves on both thighs, boots on
    /// both shins).
    pub const fn visual(self) -> ArmorMeshVisual {
        match self {
            // Head pieces: a single shell primitive that sits at the head region
            // in root-local space, so it attaches to the Body (there is no Head
            // rig part, the head is baked into the Body mesh).
            ArmorMesh::PaddedHood => {
                ArmorMeshVisual::helm("items/padded_hood/model.glb", ArmorMaterial::Cloth)
            }
            ArmorMesh::LamellarHelm => {
                ArmorMeshVisual::helm("items/lamellar_helm/model.glb", ArmorMaterial::WoodSlat)
            }
            ArmorMesh::IronHelm => {
                ArmorMeshVisual::helm("items/iron_helm/model.glb", ArmorMaterial::Steel)
            }
            // Chest pieces: a torso shell on the Body plus the symmetric aux cap
            // attached at BOTH upper arms.
            ArmorMesh::PaddedTunic => {
                ArmorMeshVisual::chest("items/padded_tunic/model.glb", ArmorMaterial::Cloth)
            }
            ArmorMesh::LamellarVest => {
                ArmorMeshVisual::chest("items/lamellar_vest/model.glb", ArmorMaterial::WoodSlat)
            }
            ArmorMesh::IronCuirass => {
                ArmorMeshVisual::chest("items/iron_cuirass/model.glb", ArmorMaterial::Steel)
            }
            // Leg pieces: one shell attached at BOTH thighs.
            ArmorMesh::PaddedLeggings => {
                ArmorMeshVisual::legs("items/padded_leggings/model.glb", ArmorMaterial::Cloth)
            }
            ArmorMesh::LamellarGreaves => {
                ArmorMeshVisual::legs("items/lamellar_greaves/model.glb", ArmorMaterial::WoodSlat)
            }
            ArmorMesh::IronGreaves => {
                ArmorMeshVisual::legs("items/iron_greaves/model.glb", ArmorMaterial::Steel)
            }
            // Feet pieces: one shell attached at BOTH shins.
            ArmorMesh::PaddedWraps => {
                ArmorMeshVisual::feet("items/padded_wraps/model.glb", ArmorMaterial::Cloth)
            }
            ArmorMesh::LamellarBoots => {
                ArmorMeshVisual::feet("items/lamellar_boots/model.glb", ArmorMaterial::WoodSlat)
            }
            ArmorMesh::IronBoots => {
                ArmorMeshVisual::feet("items/iron_boots/model.glb", ArmorMaterial::Steel)
            }
        }
    }
}

/// Which shared material family a worn-armor layer binds. Armor matches the
/// player rig's material family (PBR `StandardMaterial`, not the cel/toon family
/// the held tools use), so each family resolves to one `StandardMaterial`
/// handle built once from the detail textures. COLOR_0 on the glb carries
/// identity; the texture only adds surface grain, exactly how the rig itself
/// renders. Keeping the family as data means a future cel flip of the whole
/// player family is a change here, not in the per-piece table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorMaterial {
    /// Matte woven cloth (padded set): `assets/textures/tools/cloth.png`.
    Cloth,
    /// Matte hewn-wood slats (lamellar set): `assets/textures/props/wood_slat.png`.
    WoodSlat,
    /// Forged steel plate (iron set): `assets/textures/tools/steel.png`.
    Steel,
}

/// Which rig joint a worn-armor primitive attaches to. Every shell is authored
/// pivot-local for an identity transform at its joint, so the client only needs
/// to pick the `ChildOf` parent (no per-piece offset). The `*Both` variants
/// attach the same authored-symmetric mesh at both the left and the right joint;
/// the aux cap is likewise symmetric in X so `UpperArmsBoth` needs no mirroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorJoint {
    /// The Body part (helmets at the head region, chest shells at the torso).
    /// There is no Head rig part; the head is baked into the Body mesh.
    Body,
    /// Both upper arms (the chest piece's symmetric shoulder/pauldron cap).
    UpperArmsBoth,
    /// Both thighs (leg pieces).
    ThighsBoth,
    /// Both shins (feet pieces).
    ShinsBoth,
}

/// One attached layer of a worn-armor piece: which glb primitive it draws, the
/// material family it binds, and the rig joint(s) it parents under. A chest
/// piece has three such layers (torso shell on the Body, aux cap on each upper
/// arm, encoded as one `UpperArmsBoth` layer the client fans out); every other
/// piece has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmorLayerSpec {
    /// Primitive index into (mesh 0 of) the glb: 0 = shell, 1 = aux cap.
    pub primitive: usize,
    pub material: ArmorMaterial,
    pub joint: ArmorJoint,
}

/// The full rig visual for an [`ArmorMesh`]: its glb path plus up to two layer
/// specs (a shell, and for chest pieces a shoulder aux). Fixed capacity, no
/// allocation, since no piece needs more than a shell + aux.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArmorMeshVisual {
    /// `embedded_asset_path`-relative glb path (no `embedded://` prefix), shared
    /// by every layer (they differ only in primitive index).
    pub glb: &'static str,
    layers: [Option<ArmorLayerSpec>; 2],
}

impl ArmorMeshVisual {
    /// A helmet: one shell primitive at the Body's head region.
    const fn helm(glb: &'static str, material: ArmorMaterial) -> Self {
        Self::single(glb, material, ArmorJoint::Body)
    }

    /// A leg piece: one shell primitive mirrored across both thighs.
    const fn legs(glb: &'static str, material: ArmorMaterial) -> Self {
        Self::single(glb, material, ArmorJoint::ThighsBoth)
    }

    /// A feet piece: one shell primitive mirrored across both shins.
    const fn feet(glb: &'static str, material: ArmorMaterial) -> Self {
        Self::single(glb, material, ArmorJoint::ShinsBoth)
    }

    /// A chest piece: primitive 0 the torso shell on the Body, primitive 1 the
    /// symmetric aux cap on both upper arms.
    const fn chest(glb: &'static str, material: ArmorMaterial) -> Self {
        Self {
            glb,
            layers: [
                Some(ArmorLayerSpec {
                    primitive: 0,
                    material,
                    joint: ArmorJoint::Body,
                }),
                Some(ArmorLayerSpec {
                    primitive: 1,
                    material,
                    joint: ArmorJoint::UpperArmsBoth,
                }),
            ],
        }
    }

    const fn single(glb: &'static str, material: ArmorMaterial, joint: ArmorJoint) -> Self {
        Self {
            glb,
            layers: [
                Some(ArmorLayerSpec {
                    primitive: 0,
                    material,
                    joint,
                }),
                None,
            ],
        }
    }

    /// The layer specs of this visual, in draw order.
    pub fn layers(&self) -> impl Iterator<Item = ArmorLayerSpec> + '_ {
        self.layers.iter().flatten().copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTint {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ItemTint {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every [`HeldMesh`] variant must have at least one visual layer, or it
    /// renders nothing in-hand. A future variant added without a
    /// [`HeldMesh::visual`] arm cannot compile the exhaustive match, and one
    /// added without any layer fails here, so a missing row breaks CI rather
    /// than shipping an invisible item.
    #[test]
    fn every_held_mesh_has_a_visual_row() {
        for &mesh in HeldMesh::ALL {
            let visual = mesh.visual();
            assert!(
                visual.layers().next().is_some(),
                "{mesh:?} has no visual layers"
            );
        }
    }

    /// Every [`HeldMesh`] variant must resolve to a carry [`HeldGrip`] too, so
    /// the renderer's hand-transform is fully data-driven off the item registry.
    /// The exhaustive match in [`HeldMesh::grip`] already forces a new variant to
    /// pick an archetype; this pins that `grip()` is total over `ALL` so a mesh
    /// can never reach the renderer without a carry pose.
    #[test]
    fn every_held_mesh_has_a_grip_archetype() {
        for &mesh in HeldMesh::ALL {
            // `grip()` is a total function; calling it for every variant proves
            // none panic and each maps to one of the bounded archetypes.
            let grip = mesh.grip();
            assert!(
                matches!(
                    grip,
                    HeldGrip::Silhouette
                        | HeldGrip::LongHafted
                        | HeldGrip::Mallet
                        | HeldGrip::Bow
                        | HeldGrip::Spear
                ),
                "{mesh:?} resolved to an unexpected grip {grip:?}"
            );
        }
    }

    /// Round-trip every [`ItemModel`] variant through postcard, the wire codec,
    /// so the swing/impact identity that now rides `SwingStart`/`PlayerImpact`/
    /// `PlayerAction` survives serialisation for every archetype (tools, weapons,
    /// and the bag/deployable fallbacks alike). A new variant is a compile error
    /// in `ALL` until listed, so this covers the whole enum.
    #[test]
    fn item_model_round_trips_through_postcard() {
        for &model in ItemModel::ALL {
            let bytes = postcard::to_allocvec(&model).expect("serialize ItemModel");
            let decoded: ItemModel = postcard::from_bytes(&bytes).expect("deserialize ItemModel");
            assert_eq!(decoded, model, "{model:?} did not round-trip");
        }
    }

    /// The wire default is the empty-hand / non-combat archetype ([`ItemModel::Bag`]),
    /// so a freshly-spawned peer `PlayerAction` reads as a punch and a weapon whose
    /// wire identity ever failed to resolve degrades to the bag swing, never a panic.
    #[test]
    fn item_model_default_is_bag() {
        assert_eq!(ItemModel::default(), ItemModel::Bag);
    }

    /// Every [`ArmorMesh`] variant must have at least one attached layer, or it
    /// renders nothing on the rig. The exhaustive match in [`ArmorMesh::visual`]
    /// forces a new variant to add a row; this pins that each row produces a
    /// layer, so a missing/empty row breaks CI rather than shipping an invisible
    /// worn piece.
    #[test]
    fn every_armor_mesh_has_a_visual_row() {
        for &mesh in ArmorMesh::ALL {
            let visual = mesh.visual();
            assert!(
                visual.layers().next().is_some(),
                "{mesh:?} has no armor layers"
            );
            // The glb path is the shared per-piece asset; it must be non-empty
            // and point at the item's model.
            assert!(visual.glb.ends_with("model.glb"), "{mesh:?} glb path");
        }
    }

    /// The attachment layout is a pure function of the mesh, so pin the joint
    /// list each piece resolves to (the ART CONTRACT): a chest piece yields three
    /// attachments (torso shell on the Body plus a shoulder aux on each upper
    /// arm, encoded as one `UpperArmsBoth` layer the client fans out to two);
    /// every other piece yields a single shell layer at its joint. This is the
    /// rig-agnostic half of the attachment system, testable without a running app.
    #[test]
    fn armor_attachment_layout_matches_the_art_contract() {
        // A chest piece: shell on the Body, aux cap on both upper arms.
        for chest in [
            ArmorMesh::PaddedTunic,
            ArmorMesh::LamellarVest,
            ArmorMesh::IronCuirass,
        ] {
            let joints: Vec<ArmorJoint> = chest.visual().layers().map(|l| l.joint).collect();
            assert_eq!(
                joints,
                vec![ArmorJoint::Body, ArmorJoint::UpperArmsBoth],
                "{chest:?} must attach a torso shell (prim 0) plus a shoulder aux (prim 1)"
            );
            // Primitive indices are shell=0, aux=1 in that order.
            let prims: Vec<usize> = chest.visual().layers().map(|l| l.primitive).collect();
            assert_eq!(prims, vec![0, 1], "{chest:?} prim order");
        }

        // Head pieces: one shell on the Body (there is no Head rig part).
        for helm in [
            ArmorMesh::PaddedHood,
            ArmorMesh::LamellarHelm,
            ArmorMesh::IronHelm,
        ] {
            let joints: Vec<ArmorJoint> = helm.visual().layers().map(|l| l.joint).collect();
            assert_eq!(
                joints,
                vec![ArmorJoint::Body],
                "{helm:?} is a single body shell"
            );
        }

        // Leg pieces: one shell across both thighs.
        for legs in [
            ArmorMesh::PaddedLeggings,
            ArmorMesh::LamellarGreaves,
            ArmorMesh::IronGreaves,
        ] {
            let joints: Vec<ArmorJoint> = legs.visual().layers().map(|l| l.joint).collect();
            assert_eq!(
                joints,
                vec![ArmorJoint::ThighsBoth],
                "{legs:?} is a thighs shell"
            );
        }

        // Feet pieces: one shell across both shins.
        for feet in [
            ArmorMesh::PaddedWraps,
            ArmorMesh::LamellarBoots,
            ArmorMesh::IronBoots,
        ] {
            let joints: Vec<ArmorJoint> = feet.visual().layers().map(|l| l.joint).collect();
            assert_eq!(
                joints,
                vec![ArmorJoint::ShinsBoth],
                "{feet:?} is a shins shell"
            );
        }
    }

    /// Round-trip every [`ArmorMesh`] through postcard, the wire codec: the
    /// selectors ride `PlayerEquipmentVisual`, so every variant (including the
    /// appended lamellar and iron sets) must survive serialisation. The
    /// exhaustive `ALL` list makes a new variant a compile error until listed.
    #[test]
    fn armor_mesh_round_trips_through_postcard() {
        for &mesh in ArmorMesh::ALL {
            let bytes = postcard::to_allocvec(&mesh).expect("serialize ArmorMesh");
            let decoded: ArmorMesh = postcard::from_bytes(&bytes).expect("deserialize ArmorMesh");
            assert_eq!(decoded, mesh, "{mesh:?} did not round-trip");
        }
    }

    /// `HeldMesh::ALL` must list every variant exactly once so the completeness
    /// test above actually covers all of them. The exhaustive match here forces a
    /// new variant to be added to `ALL`, and the count guards against a duplicate
    /// or a dropped entry.
    #[test]
    fn held_mesh_all_lists_every_variant() {
        // The exhaustive match makes adding a variant a compile error until it is
        // slotted into `ALL` and given the count below.
        let expected = |mesh: HeldMesh| match mesh {
            HeldMesh::Bag
            | HeldMesh::StoneHatchet
            | HeldMesh::IronHatchet
            | HeldMesh::StonePickaxe
            | HeldMesh::IronPickaxe
            | HeldMesh::Hammer
            | HeldMesh::BuildingPlan
            | HeldMesh::WoodenClub
            | HeldMesh::StoneSpear
            | HeldMesh::IronSword
            | HeldMesh::WoodenBow
            | HeldMesh::Crossbow
            | HeldMesh::Arrow
            | HeldMesh::PowderBomb
            | HeldMesh::PowderKeg
            | HeldMesh::SatchelCharge
            | HeldMesh::Bandage
            | HeldMesh::Sickle => true,
        };
        assert!(HeldMesh::ALL.iter().all(|&mesh| expected(mesh)));
        assert_eq!(HeldMesh::ALL.len(), 18);
    }
}
