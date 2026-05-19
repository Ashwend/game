use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*};

pub(crate) fn low_poly_bag_mesh() -> Mesh {
    let bottom = [
        [-0.07, -0.09, -0.05],
        [0.07, -0.09, -0.05],
        [0.09, -0.09, 0.02],
        [0.04, -0.09, 0.075],
        [-0.05, -0.09, 0.065],
        [-0.09, -0.09, 0.00],
    ];
    let belly = [
        [-0.10, -0.01, -0.075],
        [0.10, -0.01, -0.075],
        [0.12, -0.01, 0.02],
        [0.05, -0.01, 0.105],
        [-0.07, -0.01, 0.09],
        [-0.115, -0.01, -0.005],
    ];
    let shoulder = [
        [-0.08, 0.065, -0.06],
        [0.08, 0.065, -0.06],
        [0.095, 0.065, 0.015],
        [0.04, 0.065, 0.08],
        [-0.05, 0.065, 0.07],
        [-0.09, 0.065, -0.005],
    ];
    let neck = [
        [-0.032, 0.12, -0.022],
        [0.032, 0.12, -0.022],
        [0.04, 0.12, 0.012],
        [0.014, 0.12, 0.04],
        [-0.02, 0.12, 0.034],
        [-0.04, 0.12, 0.0],
    ];
    let top = [
        [-0.022, 0.145, -0.014],
        [0.022, 0.145, -0.014],
        [0.028, 0.145, 0.008],
        [0.01, 0.145, 0.026],
        [-0.014, 0.145, 0.022],
        [-0.028, 0.145, 0.0],
    ];

    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    for ring in [&bottom, &belly, &shoulder, &neck, &top] {
        for vertex in ring {
            positions.push(*vertex);
            uvs.push([0.0, 0.0]);
        }
    }

    for ring_index in 0..4 {
        let lower = ring_index * 6;
        let upper = (ring_index + 1) * 6;
        for side in 0..6 {
            let next = (side + 1) % 6;
            indices.extend_from_slice(&[
                (lower + side) as u32,
                (lower + next) as u32,
                (upper + side) as u32,
                (upper + side) as u32,
                (lower + next) as u32,
                (upper + next) as u32,
            ]);
        }
    }

    let bottom_center = positions.len() as u32;
    positions.push([0.0, -0.09, 0.0]);
    uvs.push([0.5, 0.0]);
    for side in 0..6 {
        indices.extend_from_slice(&[bottom_center, ((side + 1) % 6) as u32, side as u32]);
    }

    let top_center = positions.len() as u32;
    positions.push([0.0, 0.15, 0.006]);
    uvs.push([0.5, 1.0]);
    for side in 0..6 {
        let next = (side + 1) % 6;
        indices.extend_from_slice(&[top_center, 24 + side as u32, 24 + next as u32]);
    }

    let outward_indices = indices
        .chunks_exact(3)
        .flat_map(|triangle| [triangle[0], triangle[2], triangle[1]])
        .collect::<Vec<_>>();
    let flat_positions = outward_indices
        .iter()
        .map(|index| positions[*index as usize])
        .collect::<Vec<_>>();
    let flat_uvs = outward_indices
        .iter()
        .map(|index| uvs[*index as usize])
        .collect::<Vec<_>>();

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, flat_positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, flat_uvs)
    .with_computed_flat_normals()
}
