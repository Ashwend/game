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
//! instance buffer (see [`super::GrassState`]) carrying `NoFrustumCulling`, drawn
//! whole. Many entities sharing one mesh collide with Bevy's automatic
//! instancing/batching, and per-region frustum culling made the field flicker
//! chunk-by-chunk as the camera moved, so the streamer keeps one combined buffer and
//! [`queue_grass`] submits it to every world view (those that can see render layer 0,
//! so never the layer-1 viewmodel camera) without culling (the shader's distance
//! dither thins the far edge).
//!
//! Perf: the combined buffer is extracted ([`extract_grass`]) and uploaded
//! ([`prepare_instance_buffers`]) **only when it changes** (tiles stream in/out),
//! not every frame, and the GPU buffer is a persistent grow-only allocation written
//! with `queue.write_buffer` (walking changes the tile set nearly continuously;
//! allocating a fresh multi-MB buffer for each change was measurable render-thread
//! churn). The field entity carries `SyncToRenderWorld` (added at spawn) so it has a
//! `RenderEntity` for the change-gated extract to target, the material-less entity
//! would not sync otherwise.

use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::RenderLayers,
    core_pipeline::core_3d::{Transparent3d, TransparentSortingInfo3d},
    ecs::system::{SystemParamItem, lifetimeless::*},
    image::{
        CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler,
        ImageSamplerDescriptor, ImageType,
    },
    mesh::{MeshVertexBufferLayoutRef, VertexBufferLayout},
    pbr::{
        MeshPipeline, MeshPipelineKey, MeshPipelineSystems, RenderMeshInstances, SetMeshBindGroup,
        SetMeshViewBindGroup, SetMeshViewBindingArrayBindGroup, ViewKeyCache,
    },
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        mesh::{RenderMesh, RenderMeshBufferInfo, allocator::MeshAllocator},
        render_asset::RenderAssets,
        render_phase::{
            AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
            RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
        },
        render_resource::{
            binding_types::{sampler, texture_2d, uniform_buffer},
            *,
        },
        renderer::{RenderDevice, RenderQueue},
        sync_world::{MainEntity, RenderEntity},
        texture::GpuImage,
        view::ExtractedView,
    },
};
use bytemuck::{Pod, Zeroable};
use std::sync::Arc;

use crate::app::embedded_asset_path;
use crate::app::embedded_assets::embedded_bytes;
use crate::app::scene::terrain::build_mip_chain;

/// Per-blade instance record. Three `vec4`s (48 bytes) fed to the vertex shader
/// as instance-step vertex attributes at `@location(3)` / `(4)` / `(6)`.
///
/// - `a = [world_x, world_z, base_y, height_scale]`
/// - `b = [yaw, shade, atlas_cell, thin_key]`
/// - `c = [tint_r, tint_g, tint_b, _]`
///
/// `world_x/world_z` are already world space (tile centre + cardinal rotation +
/// local blade offset, baked CPU-side); `yaw` is the blade's absolute spin about
/// +Y; `shade` is a per-blade brightness jitter; `atlas_cell` (0..5) selects which
/// tuft of the 3x2 card atlas this blade draws (the shader remaps the UV into that
/// cell); `thin_key` is the stable per-card key the fragment uses for the distance
/// dither dissolve; `c` is the per-blade biome colour tint (the baked blade is one
/// neutral green; `super::tile_world_instances` grades it toward the local biome).
#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(super) struct InstanceData {
    pub(super) a: [f32; 4],
    pub(super) b: [f32; 4],
    pub(super) c: [f32; 4],
}

/// Component on the grass field entity holding every visible blade. Copied into
/// the render world by [`extract_grass`] (only when it changes) and uploaded to a
/// GPU buffer by [`prepare_instance_buffers`].
///
/// The blade list is held in an [`Arc`] so the change-gated [`extract_grass`]
/// clones a 16-byte handle into the render world rather than re-copying the whole
/// multi-MB `Vec` (the streamer already moved one copy out of the per-tile rebuild;
/// extract would otherwise allocate + copy a second). Both worlds share the same
/// immutable buffer; the GPU upload reads it once.
///
/// The field entity must also carry `SyncToRenderWorld` (added at spawn): it has
/// no `Material`, so nothing else opts it into render-world sync, and without a
/// `RenderEntity` the extract below finds nothing (grass would render to an
/// off-screen capture but not the live window).
#[derive(Component, Deref)]
pub(super) struct InstanceMaterialData(pub(super) Arc<Vec<InstanceData>>);

/// Embedded path of the instanced-grass shader.
const GRASS_INSTANCED_SHADER_PATH: &str = "shaders/grass_instanced.wgsl";
/// Embedded path of the grass-card tuft ATLAS (a 3x2 grid of 6 toony tuft
/// variants; each blade draws a random cell via its instance `atlas_cell`).
const GRASS_CARD_TEXTURE_PATH: &str = "textures/grass_atlas.png";

/// The shared grass-card texture handle (decoded + mipped at startup). Extracted
/// into the render world so [`prepare_grass_bind_group`] can build the group(3)
/// texture bind group once the GPU image is ready.
#[derive(Resource, Clone, ExtractResource)]
pub(crate) struct GrassCardTexture(pub(crate) Handle<Image>);

/// The prepared group(3) bind group (tuft texture + sampler + dev flags) for the
/// grass cards. Built once in the render world; the draw skips until it exists.
#[derive(Resource)]
struct GrassCardBindGroup(BindGroup);

/// Live grass debug toggles from the `Dev` options tab, packed into a bitfield
/// (`state::grass_dev_bits`; a SET bit DISABLES that stage). `0` (the default
/// everywhere, and the only value in shipped builds) renders normally. Extracted
/// to the render world and uploaded to a small uniform the grass shader reads.
#[derive(Resource, Clone, Copy, Default, ExtractResource)]
pub(crate) struct GrassDevFlags(pub(crate) u32);

/// Render-world uniform buffer holding [`GrassDevFlags`] (padded to a vec4 for
/// alignment). Created once, rewritten each frame so toggles apply live.
#[derive(Resource)]
struct GrassDevFlagsBuffer(Buffer);

/// (Re)write the grass dev-flags uniform each frame from the extracted resource.
fn prepare_grass_dev_flags(
    mut commands: Commands,
    flags: Res<GrassDevFlags>,
    existing: Option<Res<GrassDevFlagsBuffer>>,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    // Pad the u32 into a vec4<u32> (16 bytes) to satisfy uniform-buffer alignment.
    let data: [u32; 4] = [flags.0, 0, 0, 0];
    let bytes: &[u8] = bytemuck::cast_slice(&data);
    match existing {
        Some(buffer) => render_queue.write_buffer(&buffer.0, 0, bytes),
        None => {
            let buffer = render_device.create_buffer(&BufferDescriptor {
                label: Some("grass_dev_flags"),
                size: 16,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            render_queue.write_buffer(&buffer, 0, bytes);
            commands.insert_resource(GrassDevFlagsBuffer(buffer));
        }
    }
}

pub(crate) struct GrassInstancingPlugin;

impl Plugin for GrassInstancingPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (load_grass_card_texture, super::init_grass_card_mesh),
        )
        .init_resource::<GrassDevFlags>()
        .add_plugins(ExtractResourcePlugin::<GrassCardTexture>::default())
        .add_plugins(ExtractResourcePlugin::<GrassDevFlags>::default());
        app.sub_app_mut(RenderApp)
            .add_render_command::<Transparent3d, DrawGrass>()
            .init_resource::<SpecializedMeshPipelines<GrassInstancePipeline>>()
            // Bevy 0.19 creates `MeshPipeline` in a RenderStartup system (it was
            // FromWorld-initialized before RenderStartup in 0.18). Our init clones
            // it, so it must run after the `MeshPipelineSystems` set that builds it,
            // or `Res<MeshPipeline>` fails validation at startup.
            .add_systems(
                RenderStartup,
                init_grass_pipeline.after(MeshPipelineSystems),
            )
            .add_systems(ExtractSchedule, extract_grass)
            .add_systems(
                Render,
                (
                    queue_grass.in_set(RenderSystems::QueueMeshes),
                    prepare_instance_buffers.in_set(RenderSystems::PrepareResources),
                    prepare_grass_dev_flags.in_set(RenderSystems::PrepareResources),
                    prepare_grass_bind_group.in_set(RenderSystems::PrepareBindGroups),
                ),
            );
    }
}

/// Sampler for the grass card: clamp (one tuft per card, no wrap), trilinear +
/// anisotropic over the CPU-built mip chain so far cards resolve to a smooth soft
/// mass instead of aliasing into pixel noise (the distance "pixel hell" fix).
fn grass_card_sampler() -> ImageSamplerDescriptor {
    ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..default()
    }
}

/// Decode the embedded grass-tuft PNG, build its mip chain (Bevy 0.18 doesn't mip
/// loaded PNGs), and stash the handle. RGBA8 sRGB; alpha is the tuft silhouette.
fn load_grass_card_texture(mut images: ResMut<Assets<Image>>, mut commands: Commands) {
    let bytes = embedded_bytes(GRASS_CARD_TEXTURE_PATH)
        .unwrap_or_else(|| panic!("embedded grass texture missing: {GRASS_CARD_TEXTURE_PATH}"));
    let mut image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Descriptor(grass_card_sampler()),
        RenderAssetUsages::RENDER_WORLD,
    )
    .expect("decode grass_atlas.png");
    build_mip_chain(&mut image);
    commands.insert_resource(GrassCardTexture(images.add(image)));
}

/// Build the group(3) bind group (tuft texture + sampler + dev flags) once the GPU
/// image and the dev-flags buffer are ready. Cheap guard: skips if already built or
/// an input isn't ready. The dev-flags buffer is rewritten in place each frame
/// (see [`prepare_grass_dev_flags`]), so the cached bind group stays live.
fn prepare_grass_bind_group(
    mut commands: Commands,
    pipeline: Res<GrassInstancePipeline>,
    images: Res<RenderAssets<GpuImage>>,
    texture: Option<Res<GrassCardTexture>>,
    dev_flags: Option<Res<GrassDevFlagsBuffer>>,
    existing: Option<Res<GrassCardBindGroup>>,
    render_device: Res<RenderDevice>,
) {
    if existing.is_some() {
        return;
    }
    let Some(texture) = texture else {
        return;
    };
    let Some(dev_flags) = dev_flags else {
        return;
    };
    let Some(gpu) = images.get(&texture.0) else {
        return;
    };
    let bind_group = render_device.create_bind_group(
        "grass_card_bind_group",
        &pipeline.texture_layout,
        &BindGroupEntries::sequential((
            &gpu.texture_view,
            &gpu.sampler,
            dev_flags.0.as_entire_binding(),
        )),
    );
    commands.insert_resource(GrassCardBindGroup(bind_group));
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

#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
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
    // `Option<&RenderLayers>` is extracted onto the render-world view entity by
    // `extract_cameras`. The grass field lives on the default layer (0); a view
    // that can't see layer 0 (the layer-1 viewmodel camera) must be skipped, or
    // this hand-rolled queue, which bypasses the per-view visibility filter, would
    // draw the whole world field into the viewmodel pass on top of everything.
    views: Query<(&ExtractedView, Option<&RenderLayers>)>,
) {
    let draw_grass = transparent_3d_draw_functions.read().id::<DrawGrass>();
    let world_layers = RenderLayers::default();

    for (view, view_layers) in &views {
        if !view_layers
            .unwrap_or(&world_layers)
            .intersects(&world_layers)
        {
            continue;
        }
        let Some(transparent_phase) = transparent_render_phases.get_mut(&view.retained_view_entity)
        else {
            continue;
        };
        let Some(view_key) = view_key_cache.get(&view.retained_view_entity) else {
            continue;
        };

        // The grass field is one `NoFrustumCulling` buffer drawn whole, no per-view
        // culling (per-region frustum culling flickered chunk-by-chunk as the camera
        // moved). The shader's distance dither thins the far edge.
        for (entity, main_entity) in &material_meshes {
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                continue;
            };
            let Some(mesh) = meshes.get(mesh_instance.mesh_asset_id()) else {
                continue;
            };
            // 0.19 folds the strip-index-format bits into the key; grass is a
            // triangle list (non-strip), so the strip index format is irrelevant
            // and `None` is correct (it's ignored for non-strip topologies).
            let key = *view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    None,
                );
            let pipeline = pipelines
                .specialize(&pipeline_cache, &grass_pipeline, key, &mesh.layout)
                .unwrap();
            // `add_transient`: the item is re-queued every frame (this system runs
            // in QueueMeshes) and dropped after the frame, matching the old `.add`.
            transparent_phase.add_transient(Transparent3d {
                // Draw the field FIRST among transparent items. The cards write depth
                // (see `specialize`), so drawing the effectively-opaque grass before the
                // genuinely-translucent transparent objects lets those depth-test against
                // it correctly (a particle behind a blade is occluded). In 0.19 the sort
                // distance is derived from `sorting_info`: `AlwaysOnTop` yields
                // `f32::NEG_INFINITY`, and the phase sorts ASCENDING (core_3d), so the
                // grass sorts before every real transparent, the same "drawn first"
                // result the old `distance: f32::MIN` produced. `distance` is filled in
                // later by the phase's `recalculate_sort_keys`, so the 0.0 seed is inert.
                sorting_info: TransparentSortingInfo3d::AlwaysOnTop,
                distance: 0.0,
                entity: (entity, *main_entity),
                pipeline,
                draw_function: draw_grass,
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

/// Headroom factor for a fresh allocation: capacity for the current
/// blade count plus 25%. While the player walks, tiles stream in and
/// out nearly continuously and the total count wobbles by a few tiles'
/// worth; the slack means steady movement re-uses one allocation
/// instead of creating a new multi-MB buffer several times a second.
fn grown_capacity_bytes(len_bytes: usize) -> u64 {
    (len_bytes + len_bytes / 4 + size_of::<InstanceData>()) as u64
}

fn prepare_instance_buffers(
    mut commands: Commands,
    // Only touch the GPU buffer when the extracted data changed; the retained
    // render world keeps the old `InstanceBuffer` otherwise. `extract_grass` only
    // re-inserts on change, so this fires only when the field actually changed.
    mut query: Query<
        (Entity, &InstanceMaterialData, Option<&mut InstanceBuffer>),
        Changed<InstanceMaterialData>,
    >,
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
) {
    for (entity, instance_data, existing) in &mut query {
        let bytes: &[u8] = bytemuck::cast_slice(instance_data.as_slice());
        // Reuse the existing allocation whenever it still fits: a tile
        // streaming in/out changes the byte length by a fraction of a
        // percent, and `write_buffer` is a staged copy while
        // `create_buffer_with_data` is a fresh allocation + upload of
        // the full multi-MB field every time. The draw limits itself to
        // `length` instances, so trailing stale bytes are never read.
        match existing {
            Some(mut instance_buffer) if instance_buffer.buffer.size() >= bytes.len() as u64 => {
                if !bytes.is_empty() {
                    render_queue.write_buffer(&instance_buffer.buffer, 0, bytes);
                }
                instance_buffer.length = instance_data.len();
            }
            _ => {
                if bytes.is_empty() {
                    // No blades and no (or too-small) buffer: nothing to
                    // draw; `DrawGrassInstanced` skips entities without an
                    // `InstanceBuffer`.
                    continue;
                }
                let buffer = render_device.create_buffer(&BufferDescriptor {
                    label: Some("grass instance data buffer"),
                    size: grown_capacity_bytes(bytes.len()),
                    usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                render_queue.write_buffer(&buffer, 0, bytes);
                commands.entity(entity).insert(InstanceBuffer {
                    buffer,
                    length: instance_data.len(),
                });
            }
        }
    }
}

/// group(3) layout entries: the grass-card tuft texture + sampler. Built from one
/// place so the concrete [`BindGroupLayout`] (for the bind group) and the
/// [`BindGroupLayoutDescriptor`] pushed in `specialize` (for the pipeline) match.
/// Safe to add to a hand-rolled MeshPipeline specialization on Metal (the
/// @binding(100) crash is exclusive to ExtendedMaterial's bindless merge;
/// TerrainMaterial ships the same standalone group-3 texture binding here).
fn grass_texture_layout_entries() -> Vec<BindGroupLayoutEntry> {
    // VERTEX_FRAGMENT (not FRAGMENT): the dev-flags uniform at binding 2 is read in
    // the vertex stage too (the wind toggle), so the whole group is visible to both.
    BindGroupLayoutEntries::sequential(
        ShaderStages::VERTEX_FRAGMENT,
        (
            texture_2d(TextureSampleType::Float { filterable: true }),
            sampler(SamplerBindingType::Filtering),
            uniform_buffer::<UVec4>(false),
        ),
    )
    .to_vec()
}

const GRASS_TEXTURE_LAYOUT_LABEL: &str = "grass_card_texture_layout";

#[derive(Resource)]
struct GrassInstancePipeline {
    shader: Handle<Shader>,
    mesh_pipeline: MeshPipeline,
    /// Concrete group(3) layout, used to build the bind group in the render world.
    texture_layout: BindGroupLayout,
}

fn init_grass_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mesh_pipeline: Res<MeshPipeline>,
    render_device: Res<RenderDevice>,
) {
    let texture_layout = render_device
        .create_bind_group_layout(GRASS_TEXTURE_LAYOUT_LABEL, &grass_texture_layout_entries());

    commands.insert_resource(GrassInstancePipeline {
        shader: asset_server.load(embedded_asset_path(GRASS_INSTANCED_SHADER_PATH)),
        mesh_pipeline: mesh_pipeline.clone(),
        texture_layout,
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

        // group(3): the grass-card tuft texture + sampler, appended after the
        // inherited view(0)/view-binding-array(1)/mesh(2) layouts. The descriptor's
        // entries match `texture_layout` (both from `grass_texture_layout_entries`),
        // so the bind group built against `texture_layout` is wgpu-compatible here.
        descriptor.layout.push(BindGroupLayoutDescriptor::new(
            GRASS_TEXTURE_LAYOUT_LABEL,
            &grass_texture_layout_entries(),
        ));

        // Alpha-to-coverage: the fragment outputs the tuft texture's alpha as
        // coverage, which MSAA turns into fractional, sort-free per-sample
        // coverage, so card edges anti-alias softly. Pairs with a low-threshold
        // discard so it still reads under MSAA-off (FXAA). Sample count inherited
        // from the view key.
        descriptor.multisample.alpha_to_coverage_enabled = true;

        // Keep the inherited opaque depth WRITE on. The cards are alpha-tested
        // cutouts (hard `discard` below ALPHA_CUTOFF, alpha only feeds
        // alpha-to-coverage), i.e. effectively opaque, so writing depth is the
        // standard alpha-tested-foliage setup: a near blade then occludes the far
        // blades behind it in the depth buffer, so the GPU early-Z-rejects their
        // (expensive: shadow + atmosphere IBL) fragments instead of shading the
        // whole overlapping field. Without it (the old "depth-write off" hack) no
        // blade ever occluded another and the entire ring paid full overdraw every
        // frame. Trade-off: grass now also occludes transparent objects behind it
        // (the placement ghost, ground particles), which is physically correct.
        // Double-sided: a blade is a one-sided ribbon, so back-face culling makes
        // every blade whose random yaw points away from the camera vanish, leaving
        // bald patches across the field (you see "through" half the grass). Render
        // both faces; the baked up-biased normal lights both sides the same, which
        // is exactly what we want for thin lit-from-above grass.
        descriptor.primitive.cull_mode = None;

        descriptor.vertex.shader = self.shader.clone();
        // Instance-step buffer appended after the mesh's vertex buffers.
        // Locations 0-2 are the blade mesh Position/Normal/UV and 5 its COLOR (from
        // the base layout); the instance `vec4`s take the free locations 3 and 4.
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
                // Per-blade biome colour tint (`c`). Location 5 is the mesh COLOR,
                // so the third instance vec4 takes the next free location, 6.
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: VertexFormat::Float32x4.size() * 2,
                    shader_location: 6,
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
    SetGrassCardBindGroup<3>,
    DrawGrassInstanced,
);

/// Bind the group(3) grass-card texture. Skips the draw until the bind group is
/// built (the texture's GPU image isn't ready for the first frame or two).
struct SetGrassCardBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetGrassCardBindGroup<I> {
    type Param = Option<SRes<GrassCardBindGroup>>;
    type ViewQuery = ();
    type ItemQuery = ();

    #[inline]
    fn render<'w>(
        _item: &P,
        _view: (),
        _entity: Option<()>,
        bind_group: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(bind_group) = bind_group else {
            return RenderCommandResult::Skip;
        };
        pass.set_bind_group(I, &bind_group.into_inner().0, &[]);
        RenderCommandResult::Success
    }
}

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
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_instance.mesh_asset_id()) else {
            return RenderCommandResult::Skip;
        };
        let Some(instance_buffer) = instance_buffer else {
            return RenderCommandResult::Skip;
        };
        let Some(vertex_buffer_slice) =
            mesh_allocator.mesh_vertex_slice(&mesh_instance.mesh_asset_id())
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
                    mesh_allocator.mesh_index_slice(&mesh_instance.mesh_asset_id())
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
