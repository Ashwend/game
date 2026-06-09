//! GPU-instanced detail grass: one shared blade mesh drawn once per tile with a
//! per-blade instance buffer, so each straw costs ~no extra geometry and the
//! field can be far denser than the old "bake every blade into the tile mesh"
//! path. This is the project's **only** custom render pipeline.
//!
//! Pattern: Bevy 0.18's `examples/shader_advanced/custom_shader_instancing.rs`,
//! specialising off [`MeshPipeline`] so the draw inherits the mesh-view bind
//! groups (lights, shadows, globals, and the atmosphere IBL bound on the camera
//! via `AtmosphereEnvironmentMapLight`). The shader (`grass_instanced.wgsl`)
//! then hand-builds a `PbrInput` and calls `apply_pbr_lighting`, so instanced
//! grass is lit by the *same* sun + atmosphere as the rest of the scene without
//! needing a material bind group of its own.
//!
//! Key deviation from the upstream example: instance positions are baked in
//! **world space** on the CPU (see `super::generate_layout_instances`), so the
//! shader never touches the per-entity model matrix (the example's
//! `get_world_from_local(0u)` is a single-entity hack that misindexes with many
//! tile entities). The tile entity's transform still sits at the tile centre so
//! `Transparent3d` distance sorting and mesh-instance bookkeeping stay sane.
//!
//! Draws in [`Transparent3d`] (blades are opaque with a fragment `discard` for
//! the radial fade; the pipeline keeps the standard opaque depth test/write from
//! `MeshPipeline::specialize`). Relies on `NoIndirectDrawing` already being set
//! on the camera (`src/app/scene/assets.rs`) since the draw path uses
//! `draw_indexed`, not indirect.
//!
//! **One entity, one buffer.** All visible blades live in a single entity's
//! instance buffer (see [`super::GrassState`]). Many entities sharing one mesh
//! collide with Bevy's automatic instancing/batching and render as a single
//! clumped draw, so the streamer rebuilds one combined buffer instead.
//!
//! Perf: the combined buffer is extracted ([`extract_grass`]) and uploaded
//! ([`prepare_instance_buffers`]) **only when it changes** (tiles stream in/out),
//! not every frame. The field entity carries `SyncToRenderWorld` (added at spawn)
//! so it has a `RenderEntity` for the change-gated extract to target, the
//! material-less entity would not sync otherwise.

use bevy::{
    core_pipeline::core_3d::Transparent3d,
    ecs::system::{SystemParamItem, lifetimeless::*},
    mesh::{MeshVertexBufferLayoutRef, VertexBufferLayout},
    pbr::{
        MeshPipeline, MeshPipelineKey, RenderMeshInstances, SetMeshBindGroup, SetMeshViewBindGroup,
        SetMeshViewBindingArrayBindGroup, ViewKeyCache,
    },
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::*,
        renderer::RenderDevice,
        sync_world::{MainEntity, RenderEntity},
        view::ExtractedView,
    },
};
use bytemuck::{Pod, Zeroable};

use crate::app::embedded_asset_path;

/// Per-blade instance record. Two `vec4`s (32 bytes) fed to the vertex shader as
/// instance-step vertex attributes at `@location(3)` / `@location(4)`.
///
/// - `a = [world_x, world_z, base_y, height_scale]`
/// - `b = [yaw, shade, warm, dither]`
///
/// `world_x/world_z` are already world space (tile centre + cardinal rotation +
/// local blade offset, baked CPU-side); `yaw` is the blade's absolute spin about
/// +Y; `shade`/`warm` tint the baked green; `dither` is the stable per-blade key
/// for the fragment radial fade.
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct InstanceData {
    pub(super) a: [f32; 4],
    pub(super) b: [f32; 4],
}

/// Component on the grass field entity holding every visible blade. Copied into
/// the render world by [`extract_grass`] (only when it changes) and uploaded to a
/// GPU buffer by [`prepare_instance_buffers`].
///
/// The field entity must also carry `SyncToRenderWorld` (added at spawn): it has
/// no `Material`, so nothing else opts it into render-world sync, and without a
/// `RenderEntity` the extract below finds nothing (grass would render to an
/// off-screen capture but not the live window).
#[derive(Component, Deref)]
pub(super) struct InstanceMaterialData(pub(super) Vec<InstanceData>);

/// Embedded path of the instanced-grass shader.
const GRASS_INSTANCED_SHADER_PATH: &str = "shaders/grass_instanced.wgsl";

pub(crate) struct GrassInstancingPlugin;

impl Plugin for GrassInstancingPlugin {
    fn build(&self, app: &mut App) {
        app.sub_app_mut(RenderApp)
            .add_render_command::<Transparent3d, DrawGrass>()
            .init_resource::<SpecializedMeshPipelines<GrassInstancePipeline>>()
            .add_systems(RenderStartup, init_grass_pipeline)
            .add_systems(ExtractSchedule, extract_grass)
            .add_systems(
                Render,
                (
                    queue_grass.in_set(RenderSystems::QueueMeshes),
                    prepare_instance_buffers.in_set(RenderSystems::PrepareResources),
                ),
            );
    }
}

/// Copy the field's instance list into the render world, but **only when it
/// changed** (the streamer rewrites it only as tiles load/unload). The retained
/// render world keeps the previous value otherwise, so a static field, even a
/// multi-MB one at high density, costs nothing per frame beyond the draw. Relies
/// on the field entity carrying `SyncToRenderWorld` so it has a `RenderEntity`.
fn extract_grass(
    mut commands: Commands,
    fields: Extract<Query<(RenderEntity, Ref<InstanceMaterialData>)>>,
) {
    for (render_entity, data) in &fields {
        if data.is_changed() {
            commands
                .entity(render_entity)
                .insert(InstanceMaterialData(data.0.clone()));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn queue_grass(
    transparent_3d_draw_functions: Res<DrawFunctions<Transparent3d>>,
    grass_pipeline: Res<GrassInstancePipeline>,
    mut pipelines: ResMut<SpecializedMeshPipelines<GrassInstancePipeline>>,
    pipeline_cache: Res<PipelineCache>,
    meshes: Res<RenderAssets<RenderMesh>>,
    render_mesh_instances: Res<RenderMeshInstances>,
    material_meshes: Query<(Entity, &MainEntity), With<InstanceMaterialData>>,
    mut transparent_render_phases: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    // Bevy's per-view key (msaa/hdr/atmosphere/env-map/prepass/...), so our
    // pipeline's mesh-view layout exactly matches the bind group that
    // `SetMeshViewBindGroup` will set. Re-deriving these bits by hand is fragile
    // (e.g. the camera's atmosphere IBL adds view bindings 29-31).
    view_key_cache: Res<ViewKeyCache>,
    views: Query<&ExtractedView>,
) {
    let draw_grass = transparent_3d_draw_functions.read().id::<DrawGrass>();

    for view in &views {
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view.retained_view_entity)
        else {
            continue;
        };
        let Some(view_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };
        let rangefinder = view.rangefinder3d();

        for (entity, main_entity) in &material_meshes {
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            let Some(mesh) = meshes.get(mesh_instance.mesh_asset_id) else {
                continue;
            };
            let key =
                *view_key | MeshPipelineKey::from_primitive_topology(mesh.primitive_topology());
            let pipeline = pipelines
                .specialize(&pipeline_cache, &grass_pipeline, key, &mesh.layout)
                .unwrap();
            transparent_phase.add(Transparent3d {
                entity: (entity, *main_entity),
                pipeline,
                draw_function: draw_grass,
                distance: rangefinder.distance(&mesh_instance.center),
                batch_range: 0..1,
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

#[derive(Component)]
struct InstanceBuffer {
    buffer: Buffer,
    length: usize,
}

fn prepare_instance_buffers(
    mut commands: Commands,
    // Only (re)build the GPU buffer when the extracted data changed; the retained
    // render world keeps the old `InstanceBuffer` otherwise. `extract_grass` only
    // re-inserts on change, so this fires only when the field actually changed.
    query: Query<(Entity, &InstanceMaterialData), Changed<InstanceMaterialData>>,
    render_device: Res<RenderDevice>,
) {
    for (entity, instance_data) in &query {
        let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("grass instance data buffer"),
            contents: bytemuck::cast_slice(instance_data.as_slice()),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });
        commands.entity(entity).insert(InstanceBuffer {
            buffer,
            length: instance_data.len(),
        });
    }
}

#[derive(Resource)]
struct GrassInstancePipeline {
    shader: Handle<Shader>,
    mesh_pipeline: MeshPipeline,
}

fn init_grass_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh_pipeline: Res<MeshPipeline>,
) {
    commands.insert_resource(GrassInstancePipeline {
        shader: asset_server.load(embedded_asset_path(GRASS_INSTANCED_SHADER_PATH)),
        mesh_pipeline: mesh_pipeline.clone(),
    });
}

impl SpecializedMeshPipeline for GrassInstancePipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?;

        descriptor.vertex.shader = self.shader.clone();
        // Instance-step buffer appended after the mesh's vertex buffers.
        // Locations 0-2 are the blade mesh Position/Normal/UV; the mesh's COLOR
        // sits at its usual location from the base layout.
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: size_of::<InstanceData>() as u64,
            step_mode: VertexStepMode::Instance,
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 3,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: VertexFormat::Float32x4.size(),
                    shader_location: 4,
                },
            ],
        });
        descriptor.fragment.as_mut().unwrap().shader = self.shader.clone();
        Ok(descriptor)
    }
}

type DrawGrass = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    DrawGrassInstanced,
);

struct DrawGrassInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawGrassInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>,
        SRes<RenderMeshInstances>,
        SRes<MeshAllocator>,
    );
    type ViewQuery = ();
    type ItemQuery = Read<InstanceBuffer>;

    #[inline]
    fn render<'w>(
        item: &P,
        _view: (),
        instance_buffer: Option<&'w InstanceBuffer>,
        (meshes, render_mesh_instances, mesh_allocator): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let mesh_allocator = mesh_allocator.into_inner();

        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip;
        };
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_instance.mesh_asset_id) else {
            return RenderCommandResult::Skip;
        };
        let Some(instance_buffer) = instance_buffer else {
            return RenderCommandResult::Skip;
        };
        let Some(vertex_buffer_slice) =
            mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id)
        else {
            return RenderCommandResult::Skip;
        };

        pass.set_vertex_buffer(0, vertex_buffer_slice.buffer.slice(..));
        pass.set_vertex_buffer(1, instance_buffer.buffer.slice(..));

        match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format,
                count,
            } => {
                let Some(index_buffer_slice) =
                    mesh_allocator.mesh_index_slice(&mesh_instance.mesh_asset_id)
                else {
                    return RenderCommandResult::Skip;
                };

                pass.set_index_buffer(index_buffer_slice.buffer.slice(..), *index_format);
                pass.draw_indexed(
                    index_buffer_slice.range.start..(index_buffer_slice.range.start + count),
                    vertex_buffer_slice.range.start as i32,
                    0..instance_buffer.length as u32,
                );
            }
            RenderMeshBufferInfo::NonIndexed => {
                pass.draw(vertex_buffer_slice.range, 0..instance_buffer.length as u32);
            }
        }
        RenderCommandResult::Success
    }
}
