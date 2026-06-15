use bevy::prelude::*;

use super::builder::{LowPolyMeshBuilder, MeshColor};

/// Local-space Y offset (relative to the network player entity's transform
/// origin, which is `PLAYER_VISUAL_CENTER_Y` above the feet) where the head
/// top sits. Used by the nametag overlay to anchor the floating label. Sits
/// just above the hair crown of the rig (see [`body_mesh`]).
pub(crate) const PLAYER_HEAD_TOP_LOCAL_Y: f32 = 0.86;

// "Wayfinder" palette. Flat-shaded stylized survivor: deep muted teal tunic,
// charcoal trousers, a terracotta scarf as the signature pop, tan leather
// glove/boot cuffs. **Linear** albedos (vertex colours bypass the sRGB decode
// `Color::srgb` applies), kept in the scene's grounded range so the figure
// never glows against the orange-grass terrain (the ground anchor is linear
// (0.027, 0.095, 0.040)). See `builder.rs` for the linear-vs-sRGB lesson.
const TUNIC: MeshColor = [0.040, 0.078, 0.075, 1.0];
const TUNIC_DARK: MeshColor = [0.028, 0.054, 0.052, 1.0];
const TROUSERS: MeshColor = [0.048, 0.046, 0.052, 1.0];
const BELT: MeshColor = [0.022, 0.020, 0.024, 1.0];
/// Slightly darker teal for the single vertical chest-seam accent.
const SEAM: MeshColor = [0.018, 0.032, 0.031, 1.0];
const SKIN: MeshColor = [0.36, 0.24, 0.17, 1.0];
const SKIN_DARK: MeshColor = [0.27, 0.18, 0.13, 1.0];
const HAIR: MeshColor = [0.060, 0.042, 0.030, 1.0];
/// Tan leather trim that frames each limb end (wrist cuffs, boot cuffs).
const CUFF_TAN: MeshColor = [0.085, 0.078, 0.066, 1.0];
const BOOT: MeshColor = [0.045, 0.040, 0.036, 1.0];
/// Near-black for the eye/visor strip and similar tiny detail faces.
const ACCENT: MeshColor = [0.020, 0.020, 0.024, 1.0];

// ---------------------------------------------------------------------------
// Skeleton joints (root-local; y = 0 is `PLAYER_VISUAL_CENTER_Y` above the
// feet, -Z is forward). Each articulating part is its own child entity whose
// mesh is built in *pivot-local* space (origin at the joint) so the animators
// only ever write the part's local rotation. Feet land at y = -0.90 and the
// hair crown tops out near +0.80 (the nametag anchor sits at +0.86).
// ---------------------------------------------------------------------------
const SHOULDER_X: f32 = 0.20;
const SHOULDER_Y: f32 = 0.46;
const UPPER_ARM_LEN: f32 = 0.26;
const FOREARM_LEN: f32 = 0.28;
const HIP_X: f32 = 0.105;
const HIP_Y: f32 = -0.14;
const THIGH_LEN: f32 = 0.36;

/// One articulated part of the rigged player. The reconciler spawns one child
/// entity per variant and tags it so the locomotion / swing animators can
/// target a specific limb. `HandAnchor` is an empty (mesh-less) node that
/// parents the held tool, so the tool rides the right forearm through a swing.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum PlayerPart {
    Body,
    UpperArmL,
    UpperArmR,
    ForearmL,
    ForearmR,
    HandAnchor,
    ThighL,
    ThighR,
    ShinL,
    ShinR,
}

/// Which shared part mesh a [`PlayerPart`] renders. Left/right limbs reuse the
/// same (symmetric) mesh, placed at mirrored joints.
#[derive(Clone, Copy)]
pub(crate) enum RigMesh {
    Body,
    UpperArm,
    Forearm,
    Thigh,
    Shin,
}

/// Spawn recipe for one rig part: where it hangs (parent + rest local
/// transform) and which mesh it draws (`None` for the hand anchor).
pub(crate) struct RigPartSpec {
    pub part: PlayerPart,
    pub parent: Option<PlayerPart>,
    pub rest: Transform,
    pub mesh: Option<RigMesh>,
}

fn spec(
    part: PlayerPart,
    parent: Option<PlayerPart>,
    translation: Vec3,
    mesh: Option<RigMesh>,
) -> RigPartSpec {
    RigPartSpec {
        part,
        parent,
        rest: Transform::from_translation(translation),
        mesh,
    }
}

/// Ordered rig hierarchy (parents always precede their children, so the
/// reconciler can spawn top-down and resolve each `ChildOf` from an
/// already-spawned entity). `parent == None` means a child of the root
/// `NetworkPlayer` entity. Rest rotations are identity, the animators own
/// every joint rotation, composing their pose onto this rest frame.
pub(crate) fn rig_layout() -> Vec<RigPartSpec> {
    use PlayerPart::*;
    vec![
        spec(Body, None, Vec3::ZERO, Some(RigMesh::Body)),
        spec(
            UpperArmL,
            Some(Body),
            Vec3::new(-SHOULDER_X, SHOULDER_Y, 0.0),
            Some(RigMesh::UpperArm),
        ),
        spec(
            UpperArmR,
            Some(Body),
            Vec3::new(SHOULDER_X, SHOULDER_Y, 0.0),
            Some(RigMesh::UpperArm),
        ),
        spec(
            ForearmL,
            Some(UpperArmL),
            Vec3::new(0.0, -UPPER_ARM_LEN, 0.0),
            Some(RigMesh::Forearm),
        ),
        spec(
            ForearmR,
            Some(UpperArmR),
            Vec3::new(0.0, -UPPER_ARM_LEN, 0.0),
            Some(RigMesh::Forearm),
        ),
        // Held tool parent: at the wrist of the right forearm, nudged forward
        // into the grip.
        spec(
            HandAnchor,
            Some(ForearmR),
            Vec3::new(0.0, -FOREARM_LEN, 0.04),
            None,
        ),
        spec(
            ThighL,
            None,
            Vec3::new(-HIP_X, HIP_Y, 0.0),
            Some(RigMesh::Thigh),
        ),
        spec(
            ThighR,
            None,
            Vec3::new(HIP_X, HIP_Y, 0.0),
            Some(RigMesh::Thigh),
        ),
        spec(
            ShinL,
            Some(ThighL),
            Vec3::new(0.0, -THIGH_LEN, 0.0),
            Some(RigMesh::Shin),
        ),
        spec(
            ShinR,
            Some(ThighR),
            Vec3::new(0.0, -THIGH_LEN, 0.0),
            Some(RigMesh::Shin),
        ),
    ]
}

/// Baked, shareable meshes for each distinct rig part. Built once in
/// `setup_scene`; the reconciler clones the handle for each spawned part.
#[derive(Clone)]
pub(crate) struct PlayerRigMeshes {
    pub(crate) body: Handle<Mesh>,
    pub(crate) upper_arm: Handle<Mesh>,
    pub(crate) forearm: Handle<Mesh>,
    pub(crate) thigh: Handle<Mesh>,
    pub(crate) shin: Handle<Mesh>,
}

impl PlayerRigMeshes {
    pub(crate) fn handle(&self, kind: RigMesh) -> Handle<Mesh> {
        match kind {
            RigMesh::Body => self.body.clone(),
            RigMesh::UpperArm => self.upper_arm.clone(),
            RigMesh::Forearm => self.forearm.clone(),
            RigMesh::Thigh => self.thigh.clone(),
            RigMesh::Shin => self.shin.clone(),
        }
    }
}

/// Build and register every rig part mesh into `meshes`.
pub(crate) fn build_player_rig_meshes(meshes: &mut Assets<Mesh>) -> PlayerRigMeshes {
    PlayerRigMeshes {
        body: meshes.add(body_mesh()),
        upper_arm: meshes.add(upper_arm_mesh()),
        forearm: meshes.add(forearm_mesh()),
        thigh: meshes.add(thigh_mesh()),
        shin: meshes.add(shin_mesh()),
    }
}

/// Torso + pelvis + head, built in root-local space (the `Body` part hangs at
/// identity off the root). A tapered V-torso (wide chest, trimmer waist) over a
/// trousered pelvis, topped by a cube head with a hair crown, a back-of-head
/// hair mass for the rear silhouette, and a dark visor strip + nose nub so the
/// facing direction reads at a glance (-Z is forward).
fn body_mesh() -> Mesh {
    let mut b = LowPolyMeshBuilder::default();
    // Chest (the wide top of the V).
    b.add_box([0.0, 0.30, 0.0], [0.20, 0.16, 0.125], TUNIC);
    // Waist (narrower, gives the taper).
    b.add_box([0.0, 0.06, 0.0], [0.15, 0.10, 0.11], TUNIC);
    // Belt seam across the waist.
    b.add_box([0.0, -0.04, 0.0], [0.155, 0.03, 0.115], BELT);
    // Pelvis (the legs emerge from here).
    b.add_box([0.0, -0.13, 0.0], [0.155, 0.075, 0.105], TROUSERS);
    // Single vertical chest-seam accent down the front (-Z is forward).
    b.add_box([0.0, 0.30, -0.124], [0.012, 0.13, 0.006], SEAM);
    // Neck.
    b.add_box([0.0, 0.49, 0.0], [0.05, 0.045, 0.05], SKIN);
    // Head.
    b.add_box([0.0, 0.64, 0.0], [0.105, 0.11, 0.10], SKIN);
    // Hair crown.
    b.add_box([0.0, 0.76, 0.0], [0.11, 0.035, 0.105], HAIR);
    // Back-of-head hair mass (reads from behind).
    b.add_box([0.0, 0.70, 0.075], [0.10, 0.07, 0.035], HAIR);
    // Visor / eye strip, front-facing dark band.
    b.add_box([0.0, 0.655, -0.10], [0.085, 0.02, 0.006], ACCENT);
    // Nose nub for an extra facing cue.
    b.add_box([0.0, 0.61, -0.105], [0.018, 0.022, 0.012], SKIN_DARK);
    b.build()
}

/// Sleeved upper arm, pivot at the shoulder, hanging down -Y.
fn upper_arm_mesh() -> Mesh {
    let mut b = LowPolyMeshBuilder::default();
    // Shoulder cap (a touch wider so the joint reads).
    b.add_box([0.0, -0.025, 0.0], [0.058, 0.045, 0.062], TUNIC);
    // Upper sleeve, slightly darker than the torso so the limb separates from
    // the body when it crosses in front during a swing.
    b.add_box([0.0, -0.16, 0.0], [0.05, 0.11, 0.052], TUNIC_DARK);
    b.build()
}

/// Forearm, pivot at the elbow: a rolled sleeve end, bare skin, a tan glove
/// cuff at the wrist, then the hand.
fn forearm_mesh() -> Mesh {
    let mut b = LowPolyMeshBuilder::default();
    // Rolled sleeve just below the elbow.
    b.add_box([0.0, -0.03, 0.0], [0.05, 0.04, 0.052], TUNIC_DARK);
    // Bare forearm.
    b.add_box([0.0, -0.15, 0.004], [0.043, 0.10, 0.046], SKIN);
    // Tan glove cuff at the wrist.
    b.add_box([0.0, -0.255, 0.004], [0.05, 0.028, 0.052], CUFF_TAN);
    // Hand.
    b.add_box([0.0, -0.30, 0.006], [0.042, 0.035, 0.046], SKIN);
    b.build()
}

/// Thigh, pivot at the hip.
fn thigh_mesh() -> Mesh {
    let mut b = LowPolyMeshBuilder::default();
    b.add_box([0.0, -0.18, 0.0], [0.072, 0.18, 0.082], TROUSERS);
    b.build()
}

/// Shin + flared boot, pivot at the knee. The boot foot points forward (-Z)
/// and the sole reaches local y = -0.40, putting the feet at root y = -0.90.
fn shin_mesh() -> Mesh {
    let mut b = LowPolyMeshBuilder::default();
    // Shin tube.
    b.add_box([0.0, -0.16, 0.004], [0.062, 0.16, 0.078], TROUSERS);
    // Flared boot cuff (tan).
    b.add_box([0.0, -0.30, 0.006], [0.082, 0.04, 0.09], CUFF_TAN);
    // Boot foot, toe pointing forward.
    b.add_box([0.0, -0.355, -0.03], [0.072, 0.045, 0.11], BOOT);
    b.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_layout_parents_precede_children() {
        let layout = rig_layout();
        let mut seen = std::collections::HashSet::new();
        for spec in &layout {
            if let Some(parent) = spec.parent {
                assert!(
                    seen.contains(&parent),
                    "parent {:?} of {:?} must be spawned first",
                    parent,
                    spec.part
                );
            }
            seen.insert(spec.part);
        }
        // Every articulating part is present exactly once.
        assert_eq!(layout.len(), 10);
    }

    #[test]
    fn hand_anchor_is_the_only_meshless_part() {
        for spec in rig_layout() {
            let has_mesh = spec.mesh.is_some();
            if spec.part == PlayerPart::HandAnchor {
                assert!(!has_mesh, "hand anchor is an empty node");
            } else {
                assert!(has_mesh, "{:?} should carry a mesh", spec.part);
            }
        }
    }

    #[test]
    fn meshes_build_without_panicking() {
        // The builders should produce non-empty meshes (positions present).
        for mesh in [
            body_mesh(),
            upper_arm_mesh(),
            forearm_mesh(),
            thigh_mesh(),
            shin_mesh(),
        ] {
            assert!(
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_some(),
                "mesh should have positions"
            );
        }
    }
}
