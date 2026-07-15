//! Rigged remote body: a hierarchy of part child entities under the root,
//! built off the `Added<NetworkPlayer>` edge and restructured only on real
//! appearance changes (held item, worn armor, corpse fade). The root stays the
//! interpolation target; the per-frame part animation lives in `locomotion`.

use std::collections::HashMap;

use bevy::{light::NotShadowCaster, prelude::*};

use crate::{
    app::{
        scene::{NetworkPlayer, PlayerPart, PlayerVisualAssets, rig_layout},
        systems::items::{
            ArmorVisuals, HeldItemVisuals, armor_layers, held_item_hand_transform,
            held_item_layers, insert_held_layer_material,
        },
    },
    items::{ArmorJoint, HeldMesh, ItemModel},
};

use super::{DyingPlayer, RemoteEquipment, RemoteHeld, RemoteHeldPiece};

/// An in-progress third-person swing on a remote body.
#[derive(Debug, Clone, Copy)]
pub(super) struct RemoteSwing {
    /// Swing animation archetype: drives the third-person arc and duration. Read
    /// straight from the peer's replicated `PlayerAction.model` (a weapon's own
    /// archetype or a gather tool's), so peers animate the right swing directly
    /// off the wire.
    pub(super) model: ItemModel,
    pub(super) elapsed: f32,
    pub(super) duration: f32,
}

/// Handles to a remote player's rig part entities plus its animation state.
/// Lives on the root NetworkPlayer entity, attached by
/// `reconcile_player_rigs_system`.
#[derive(Component)]
pub(crate) struct PlayerRig {
    pub(super) body: Entity,
    pub(super) upper_arm_l: Entity,
    pub(super) upper_arm_r: Entity,
    pub(super) forearm_l: Entity,
    pub(super) forearm_r: Entity,
    pub(super) hand_anchor: Entity,
    pub(super) thigh_l: Entity,
    pub(super) thigh_r: Entity,
    pub(super) shin_l: Entity,
    pub(super) shin_r: Entity,
    /// Held-tool layer entities parented to the hand anchor (despawned + rebuilt
    /// on a held-item change).
    pub(super) held_layers: Vec<Entity>,
    /// Last-seen replicated held mesh, for change detection (NOT `is_changed`,
    /// which lies for Lightyear-touched components).
    pub(super) last_held: Option<HeldMesh>,
    /// Worn-armor layer entities parented to the rig joints (despawned + rebuilt
    /// on an equipment change), across all four slots and both L/R mirrors.
    pub(super) armor_layers: Vec<Entity>,
    /// Last-seen replicated worn armor, for the same manual edge detection as
    /// `last_held` (NEVER `is_changed`, which lies for Lightyear-touched
    /// components).
    pub(super) last_equipment: RemoteEquipment,
    /// Last-seen swing seq, for edge detection.
    pub(super) last_swing_seq: u32,
    pub(super) swing: Option<RemoteSwing>,
    /// Accumulated walk-cycle phase.
    pub(super) stride_phase: f32,
    /// Render-smoothed look pitch. The replicated pose (hence `RemoteLocomotion::
    /// pitch`) steps at the network tick rate; this eases toward it each frame so
    /// the upper-body lean glides instead of stair-stepping, matching how the root
    /// yaw is interpolated. Seeded on first sight in the animator.
    pub(super) smoothed_pitch: f32,
    /// True once `smoothed_pitch` has been seeded from the first replicated pitch
    /// (so a peer first seen mid-look snaps to it instead of easing up from level).
    pub(super) pitch_seeded: bool,
    /// True once the rig parts have been repointed to the per-corpse fade
    /// material (so the repoint runs once per death, not every frame).
    pub(super) corpse_faded: bool,
}

/// The rig joint entity(ies) a worn-armor [`ArmorJoint`] attaches under, per the
/// P4a ART CONTRACT: helmets and chest shells parent to the Body (there is no
/// Head rig part, the head is baked into the Body mesh); the chest's symmetric
/// shoulder aux, the leg shells, and the feet shells attach to BOTH the left and
/// right joint. Returned as a small fixed-capacity list the caller iterates, so
/// a `*Both` joint fans one authored (X-symmetric) mesh out to two child
/// entities with no mirroring transform.
fn armor_joint_entities(rig: &PlayerRig, joint: ArmorJoint) -> Vec<Entity> {
    match joint {
        ArmorJoint::Body => vec![rig.body],
        ArmorJoint::UpperArmsBoth => vec![rig.upper_arm_l, rig.upper_arm_r],
        ArmorJoint::ThighsBoth => vec![rig.thigh_l, rig.thigh_r],
        ArmorJoint::ShinsBoth => vec![rig.shin_l, rig.shin_r],
    }
}

impl PlayerRig {
    /// Mesh-bearing parts (everything but the empty hand anchor), for the
    /// corpse-material repoint.
    fn mesh_parts(&self) -> [Entity; 9] {
        [
            self.body,
            self.upper_arm_l,
            self.upper_arm_r,
            self.forearm_l,
            self.forearm_r,
            self.thigh_l,
            self.thigh_r,
            self.shin_l,
            self.shin_r,
        ]
    }
}

/// Builds the part hierarchy for a freshly-spawned remote player off the
/// `Added<NetworkPlayer>` edge: the root carries no mesh, so this hangs the
/// body/limbs off it and records the part entities in `PlayerRig`. Despawn is
/// automatic, removing the root recursively removes the parts.
pub(crate) fn reconcile_player_rigs_system(
    mut commands: Commands,
    assets: Res<PlayerVisualAssets>,
    new_players: Query<Entity, Added<NetworkPlayer>>,
) {
    for root in &new_players {
        let mut parts: HashMap<PlayerPart, Entity> = HashMap::new();
        for spec in rig_layout() {
            let parent = match spec.parent {
                Some(part) => parts[&part],
                None => root,
            };
            let mut entity =
                commands.spawn((spec.part, spec.rest, Visibility::Inherited, ChildOf(parent)));
            if let Some(kind) = spec.mesh {
                entity.insert((
                    Mesh3d(assets.rig.handle(kind)),
                    MeshMaterial3d(assets.remote_material.clone()),
                ));
            }
            parts.insert(spec.part, entity.id());
        }
        commands.entity(root).insert(PlayerRig {
            body: parts[&PlayerPart::Body],
            upper_arm_l: parts[&PlayerPart::UpperArmL],
            upper_arm_r: parts[&PlayerPart::UpperArmR],
            forearm_l: parts[&PlayerPart::ForearmL],
            forearm_r: parts[&PlayerPart::ForearmR],
            hand_anchor: parts[&PlayerPart::HandAnchor],
            thigh_l: parts[&PlayerPart::ThighL],
            thigh_r: parts[&PlayerPart::ThighR],
            shin_l: parts[&PlayerPart::ShinL],
            shin_r: parts[&PlayerPart::ShinR],
            held_layers: Vec::new(),
            last_held: None,
            armor_layers: Vec::new(),
            last_equipment: RemoteEquipment::default(),
            last_swing_seq: 0,
            swing: None,
            stride_phase: 0.0,
            smoothed_pitch: 0.0,
            pitch_seeded: false,
            corpse_faded: false,
        });
    }
}

/// Structural appearance updates: swap the hand-held tool when the replicated
/// held item changes, rebuild the worn-armor layers when the replicated
/// equipment changes, and repoint the rig parts to the per-corpse fade material
/// on death (and back on respawn). Runs only on real changes, so the steady
/// state costs nothing.
pub(crate) fn apply_remote_player_appearance_system(
    mut commands: Commands,
    held_visuals: Res<HeldItemVisuals>,
    armor_visuals: Res<ArmorVisuals>,
    player_assets: Res<PlayerVisualAssets>,
    mut rigs: Query<(
        &mut PlayerRig,
        &RemoteHeld,
        &RemoteEquipment,
        Option<&DyingPlayer>,
    )>,
) {
    for (mut rig, held, equipment, dying) in &mut rigs {
        // Held-item swap.
        if held.0 != rig.last_held {
            rig.last_held = held.0;
            for entity in std::mem::take(&mut rig.held_layers) {
                commands.entity(entity).despawn();
            }
            if let Some(mesh) = held.0 {
                let grip = held_item_hand_transform(mesh);
                let anchor = rig.hand_anchor;
                for held_layer in held_item_layers(&held_visuals, mesh, false) {
                    let mut layer = commands.spawn((
                        Name::new("Held Item (remote)"),
                        Mesh3d(held_layer.mesh),
                        grip,
                        // Tag the layer's rig slot so the draw animator can flex
                        // the bow's limbs / string per-piece; `Static` pieces stay
                        // put at the whole-item grip.
                        RemoteHeldPiece(held_layer.slot),
                        Visibility::Inherited,
                        // Shadow would be noise at this scale; it rides the
                        // swinging arm anyway.
                        NotShadowCaster,
                        ChildOf(anchor),
                    ));
                    insert_held_layer_material(&mut layer, held_layer.material);
                    rig.held_layers.push(layer.id());
                }
            }
        }

        // Worn-armor swap: same manual edge detection as the held item (NEVER
        // `is_changed`, which lies for Lightyear-touched components). On any
        // change to the four worn selectors, tear down every armor layer and
        // rebuild them from the current set, parenting each shell to the joint(s)
        // the ART CONTRACT dictates. A shell is authored pivot-local for identity
        // attach, so the child transform is `IDENTITY`; the `*Both` joints attach
        // the same (X-symmetric) mesh at both the left and right joint.
        if *equipment != rig.last_equipment {
            rig.last_equipment = *equipment;
            for entity in std::mem::take(&mut rig.armor_layers) {
                commands.entity(entity).despawn();
            }
            let worn = [
                equipment.head,
                equipment.chest,
                equipment.legs,
                equipment.feet,
            ];
            for mesh in worn.into_iter().flatten() {
                for layer in armor_layers(&armor_visuals, mesh) {
                    for joint in armor_joint_entities(&rig, layer.joint) {
                        let entity = commands
                            .spawn((
                                Name::new("Armor (remote)"),
                                Mesh3d(layer.mesh.clone()),
                                MeshMaterial3d(layer.material.clone()),
                                // Shells are authored pivot-local for identity
                                // attach at their joint.
                                Transform::IDENTITY,
                                Visibility::Inherited,
                                // The rig itself is a shadow caster; the armor
                                // shells sit flush over the body parts, so their
                                // own shadow would only fight the body's. Match
                                // the held-layer choice and skip the shadow pass.
                                NotShadowCaster,
                                ChildOf(joint),
                            ))
                            .id();
                        rig.armor_layers.push(entity);
                    }
                }
            }
        }

        // Corpse fade material: the parts share the live opaque material, so a
        // death fade needs them repointed onto the per-corpse Blend clone, then
        // back to the shared one on respawn.
        match (dying, rig.corpse_faded) {
            (Some(dying), false) => {
                let material = dying.material.clone();
                for part in rig.mesh_parts() {
                    commands
                        .entity(part)
                        .insert(MeshMaterial3d(material.clone()));
                }
                rig.corpse_faded = true;
            }
            (None, true) => {
                for part in rig.mesh_parts() {
                    commands
                        .entity(part)
                        .insert(MeshMaterial3d(player_assets.remote_material.clone()));
                }
                rig.corpse_faded = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::ArmorMesh;

    /// Build a `PlayerRig` with ten distinct placeholder joint entities so the
    /// pure attachment helpers can be exercised without a running app. Only the
    /// joint entity fields matter here.
    fn test_rig() -> PlayerRig {
        let mut next = 0u32;
        let mut fresh = || {
            let entity = Entity::from_raw_u32(next).expect("valid raw entity index");
            next += 1;
            entity
        };
        PlayerRig {
            body: fresh(),
            upper_arm_l: fresh(),
            upper_arm_r: fresh(),
            forearm_l: fresh(),
            forearm_r: fresh(),
            hand_anchor: fresh(),
            thigh_l: fresh(),
            thigh_r: fresh(),
            shin_l: fresh(),
            shin_r: fresh(),
            held_layers: Vec::new(),
            last_held: None,
            armor_layers: Vec::new(),
            last_equipment: RemoteEquipment::default(),
            last_swing_seq: 0,
            swing: None,
            stride_phase: 0.0,
            smoothed_pitch: 0.0,
            pitch_seeded: false,
            corpse_faded: false,
        }
    }

    #[test]
    fn armor_joints_map_to_the_contract_rig_parts() {
        // The ART CONTRACT joint mapping, resolved to actual rig entities: helmets
        // and chest shells go on the Body (one part, there is no Head rig part);
        // the shoulder aux, legs, and feet mirror across both L/R joints.
        let rig = test_rig();
        assert_eq!(armor_joint_entities(&rig, ArmorJoint::Body), vec![rig.body]);
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::UpperArmsBoth),
            vec![rig.upper_arm_l, rig.upper_arm_r]
        );
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::ThighsBoth),
            vec![rig.thigh_l, rig.thigh_r]
        );
        assert_eq!(
            armor_joint_entities(&rig, ArmorJoint::ShinsBoth),
            vec![rig.shin_l, rig.shin_r]
        );
    }

    #[test]
    fn a_full_chest_piece_resolves_to_three_attachment_targets() {
        // The chest piece is the only one that fans out to three child entities:
        // one torso shell on the Body plus a shoulder aux on each upper arm. This
        // is the rig-entity half of the pure layout test in `items::visual`.
        let rig = test_rig();
        let visual = ArmorMesh::IronCuirass.visual();
        let targets: Vec<Entity> = visual
            .layers()
            .flat_map(|layer| armor_joint_entities(&rig, layer.joint))
            .collect();
        assert_eq!(targets, vec![rig.body, rig.upper_arm_l, rig.upper_arm_r]);
    }

    #[test]
    fn remote_equipment_edge_detection_fires_only_on_a_change() {
        // The appearance system rebuilds armor when the mirror differs from the
        // last-seen value (the `last_held` pattern, never `is_changed`). Pin that
        // the `PartialEq` on `RemoteEquipment` distinguishes a real equip from an
        // identical re-send, so a steady state never churns the layer entities.
        let bare = RemoteEquipment::default();
        let helmed = RemoteEquipment {
            head: Some(ArmorMesh::LamellarHelm),
            ..RemoteEquipment::default()
        };
        // Same value: no rebuild.
        assert_eq!(bare, RemoteEquipment::default());
        // A new worn piece: rebuild.
        assert_ne!(bare, helmed);
        // Swapping one slot's mesh is still a change.
        let iron_helmed = RemoteEquipment {
            head: Some(ArmorMesh::IronHelm),
            ..RemoteEquipment::default()
        };
        assert_ne!(helmed, iron_helmed);
    }
}
