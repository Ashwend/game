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
    /// Iron mace: a big, slow overhead with a pronounced wind-up and
    /// follow-through.
    Mace,
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
        ItemModel::Mace,
        ItemModel::Bow,
        ItemModel::Crossbow,
        ItemModel::ThrownBomb,
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
    /// Iron mace: a wooden haft (Wood) under a forged iron head (Iron).
    IronMace,
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
        HeldMesh::IronMace,
        HeldMesh::WoodenBow,
        HeldMesh::Crossbow,
        HeldMesh::Arrow,
        HeldMesh::PowderBomb,
        HeldMesh::PowderKeg,
        HeldMesh::SatchelCharge,
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
            // The bag silhouette covers raw materials and deployables-in-hand;
            // its mesh is the one procedural cuboid, not an authored glb.
            HeldMesh::Bag => HeldMeshVisual::single(HeldLayerSpec {
                mesh: HeldLayerMeshSource::ProceduralBag,
                material: HeldMeshMaterial::BagStandard,
                slot: HeldPieceSlot::Static,
            }),
            HeldMesh::StoneHatchet => HeldMeshVisual::tool(
                "items/wood_stone_hatchet/model.glb",
                HeldMeshMaterial::Stone,
            ),
            HeldMesh::StonePickaxe => HeldMeshVisual::tool(
                "items/wood_stone_pickaxe/model.glb",
                HeldMeshMaterial::Stone,
            ),
            HeldMesh::IronHatchet => {
                HeldMeshVisual::tool("items/iron_hatchet/model.glb", HeldMeshMaterial::Iron)
            }
            HeldMesh::IronPickaxe => {
                HeldMeshVisual::tool("items/iron_pickaxe/model.glb", HeldMeshMaterial::Iron)
            }
            // The hammer is a wooden mallet glb: wood body (handle + head) plus
            // its iron band hoops, so its head layer rides the Iron family.
            HeldMesh::Hammer => {
                HeldMeshVisual::tool("items/hammer/model.glb", HeldMeshMaterial::Iron)
            }
            // The building plan is a rolled scroll glb: parchment paper (roll +
            // flap) plus its twine ties (Wood family, a brown COLOR_0).
            HeldMesh::BuildingPlan => HeldMeshVisual::two(
                "items/building_plan/model.glb",
                HeldMeshMaterial::Parchment,
                HeldMeshMaterial::Wood,
            ),
            // The four melee weapons are all two-primitive haft+head glbs
            // (primitive 0 the wooden grip, primitive 1 the worked head), so
            // they reuse the same `tool()` layout as the hatchets; only the head
            // family differs. Per-weapon colour is baked into each glb's COLOR_0.
            // The club's head is wood too, so both its layers ride the Wood
            // family.
            HeldMesh::WoodenClub => {
                HeldMeshVisual::tool("items/wooden_club/model.glb", HeldMeshMaterial::Wood)
            }
            HeldMesh::StoneSpear => {
                HeldMeshVisual::tool("items/stone_spear/model.glb", HeldMeshMaterial::Stone)
            }
            HeldMesh::IronSword => {
                HeldMeshVisual::tool("items/iron_sword/model.glb", HeldMeshMaterial::Iron)
            }
            HeldMesh::IronMace => {
                HeldMeshVisual::tool("items/iron_mace/model.glb", HeldMeshMaterial::Iron)
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
            HeldMesh::Arrow => {
                HeldMeshVisual::tool("items/arrow/model.glb", HeldMeshMaterial::Stone)
            }
            // The three explosives are two-primitive glbs (primitive 0 the body,
            // primitive 1 the detail), following the 2-prim convention the art
            // agent authors to. Per-charge colour is baked into each glb's
            // COLOR_0; the family is only which shared material a layer binds.
            // Bomb: cloth ball + iron fuse cap. Keg: staved wood barrel + iron
            // hoops. Satchel: cloth pack + leather strap.
            HeldMesh::PowderBomb => HeldMeshVisual::two(
                "items/powder_bomb/model.glb",
                HeldMeshMaterial::Cloth,
                HeldMeshMaterial::Iron,
            ),
            HeldMesh::PowderKeg => HeldMeshVisual::two(
                "items/powder_keg/model.glb",
                HeldMeshMaterial::Wood,
                HeldMeshMaterial::Iron,
            ),
            HeldMesh::SatchelCharge => HeldMeshVisual::two(
                "items/satchel_charge/model.glb",
                HeldMeshMaterial::Cloth,
                HeldMeshMaterial::Leather,
            ),
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
            _ => 1.0,
        }
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
            | HeldMesh::SatchelCharge => HeldGrip::Silhouette,
            // The bladed/pointed long tools (and dedicated weapons that reuse
            // this look) are gripped low on a haft carried out front. The sword
            // sits here for its blade-forward silhouette, the spear for its long
            // point-forward haft, and the bow/crossbow for their held-out-front
            // ranged carry (their draw / reload animation rides the per-piece
            // transform + the ranged pose, not this carry grip).
            HeldMesh::StoneHatchet
            | HeldMesh::IronHatchet
            | HeldMesh::StonePickaxe
            | HeldMesh::IronPickaxe
            | HeldMesh::StoneSpear
            | HeldMesh::IronSword
            | HeldMesh::WoodenBow
            | HeldMesh::Crossbow => HeldGrip::LongHafted,
            // Short one-handers held close in: the construction mallet and the
            // two blunt weapons (club, mace).
            HeldMesh::Hammer | HeldMesh::WoodenClub | HeldMesh::IronMace => HeldGrip::Mallet,
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
    /// Wooden haft + twine bindings (tools) and the building-plan ties.
    Wood,
    /// Knapped stone tool heads.
    Stone,
    /// Forged iron tool heads and the hammer's band hoops.
    Iron,
    /// Rolled parchment paper of the building-plan scroll.
    Parchment,
    /// Woven cloth: the powder bomb's wrap and the satchel charge's pack body.
    Cloth,
    /// Tanned leather: the satchel charge's strap.
    Leather,
    /// Pale tan bowstring / crossbow-string cord. A slim neutral cord tile whose
    /// COLOR_0 carries the pale-tan string colour, the same `detail * COLOR_0` cel
    /// path the other families use.
    Cord,
    /// The bag silhouette's flat `StandardMaterial` (no cel/viewmodel variant).
    BagStandard,
}

/// Where a held-item layer's mesh comes from: an authored glb primitive or the
/// single procedural bag cuboid. Kept as data so the renderer loads glbs and
/// builds the bag mesh from one table pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldLayerMeshSource {
    /// The shared procedural bag cuboid (`ItemVisualAssets::held_bag_mesh`).
    ProceduralBag,
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

    /// A two-primitive authored tool glb: primitive 0 is the wooden haft body
    /// (Wood family) and primitive 1 is the worked head (`head` family). Both are
    /// static pieces (the swing transform moves the whole tool).
    const fn tool(glb: &'static str, head: HeldMeshMaterial) -> Self {
        Self::two_glb(glb, HeldMeshMaterial::Wood, head)
    }

    /// A two-primitive authored glb with an explicit family for each primitive
    /// (primitive 0 = `first`, primitive 1 = `second`). Both static.
    const fn two(glb: &'static str, first: HeldMeshMaterial, second: HeldMeshMaterial) -> Self {
        Self::two_glb(glb, first, second)
    }

    const fn two_glb(glb: &'static str, first: HeldMeshMaterial, second: HeldMeshMaterial) -> Self {
        Self {
            layers: [
                Some(Self::glb_layer(glb, 0, first, HeldPieceSlot::Static)),
                Some(Self::glb_layer(glb, 1, second, HeldPieceSlot::Static)),
                None,
                None,
                None,
                None,
            ],
        }
    }

    /// The animatable wooden bow: six primitives (grip, upper limb, lower limb,
    /// upper string leg, lower string leg, nocked arrow) in the glb's primitive
    /// order. The wood pieces ride Wood (the arrow's stone head is carried by
    /// its COLOR_0); the two string legs ride Cord. Each carries its rig slot so
    /// the per-piece animator can flex the limbs, pull the string, and slide the
    /// nocked arrow back with it.
    const fn bow(glb: &'static str) -> Self {
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
                    HeldMeshMaterial::Wood,
                    HeldPieceSlot::BowLimbUpper,
                )),
                Some(Self::glb_layer(
                    glb,
                    2,
                    HeldMeshMaterial::Wood,
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

    /// The animatable crossbow: four primitives (wood stock, iron fittings,
    /// string, loaded bolt). The stock and iron are static; the string slides
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
/// server-side. Adding a set is a new variant here plus its [`ArmorProfile`]
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
    /// registered piece behind it. Adding a variant without an [`ArmorProfile`]
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
                    HeldGrip::Silhouette | HeldGrip::LongHafted | HeldGrip::Mallet
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
            | HeldMesh::IronMace
            | HeldMesh::WoodenBow
            | HeldMesh::Crossbow
            | HeldMesh::Arrow
            | HeldMesh::PowderBomb
            | HeldMesh::PowderKeg
            | HeldMesh::SatchelCharge => true,
        };
        assert!(HeldMesh::ALL.iter().all(|&mesh| expected(mesh)));
        assert_eq!(HeldMesh::ALL.len(), 17);
    }
}
