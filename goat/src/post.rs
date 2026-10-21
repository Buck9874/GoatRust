//! UE3's uber post-process pieces Bevy doesn't have: depth of field and image grain from
//! WorldInfo.DefaultPostProcessSettings. A fullscreen pass before tonemapping (Bevy's
//! fullscreen_material plugin plus the camera's depth buffer, read directly so no depth prepass
//! is needed). DOF follows DOFAndBloomCommon.usf: UnfocusedPercent =
//! min(MaxNear/MaxFarBlur, pow(saturate(|depth - FocusDistance| / FocusInnerRadius), Falloff)),
//! blurred over BlurKernelSize pixels (at 720 lines); grain adds Scene_ImageGrainScale noise;
//! then the scene tone: (colour - Scene_Shadows) x Scene_HighLights, ^ Scene_MidTones,
//! Scene_Desaturation towards grey, x Scene_Colorize. PostProcessVolumes (look.rs) swap the
//! settings while the camera is inside them.

use bevy::core_pipeline::{schedule::Core3d, tonemapping::tonemapping, Core3dSystems, FullscreenShader};
use bevy::prelude::*;
use bevy::render::{
    camera::ExtractedCamera,
    extract_component::{ComponentUniforms, DynamicUniformIndex, ExtractComponent, ExtractComponentPlugin, UniformComponentPlugin},
    render_resource::{
        binding_types::{sampler, texture_2d, texture_depth_2d, texture_depth_2d_multisampled, uniform_buffer},
        BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId, ColorTargetState, ColorWrites, FragmentState, Operations, PipelineCache,
        RenderPassColorAttachment, RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, ShaderType, TextureFormat,
        TextureSampleType,
    },
    renderer::{RenderContext, RenderDevice, ViewQuery},
    view::{ExtractedView, ViewDepthTexture, ViewTarget},
    Render, RenderApp, RenderStartup, RenderSystems,
};
use bevy::shader::ShaderDefVal;

/// The map's DOF and grain settings (distances in metres), on the game camera.
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default, Debug)]
pub struct UberPost {
    pub focus_distance: f32,
    pub focus_radius: f32,
    pub falloff: f32,
    pub max_near: f32,
    pub max_far: f32,
    pub min_blur: f32,
    /// Blur radius in pixels at 720 lines; 0 = no DOF.
    pub kernel: f32,
    pub grain: f32,
    pub near_plane: f32,
    pub time: f32,
    pub desaturation: f32,
    pub shadows: Vec4,
    pub highlights: Vec4,
    pub midtones: Vec4,
    pub colorize: Vec4,
}

/// One PostProcessSettings, resolved (UE3 struct defaults for fields it doesn't set).
#[derive(Clone, Debug, PartialEq)]
pub struct PostSettings {
    pub dof: bool,
    pub focus_distance: f32,
    pub focus_radius: f32,
    pub falloff: f32,
    pub max_near: f32,
    pub max_far: f32,
    pub min_blur: f32,
    pub kernel: f32,
    pub grain: f32,
    pub desaturation: f32,
    pub shadows: Vec3,
    pub highlights: Vec3,
    pub midtones: Vec3,
    pub colorize: Vec3,
    /// DOF_InterpolationDuration: seconds to blend into these settings.
    pub blend: f32,
}

impl Default for PostSettings {
    fn default() -> Self {
        // Engine PostProcessSettings structdefaultproperties
        PostSettings {
            dof: false,
            focus_distance: 0.0,
            focus_radius: 2000.0,
            falloff: 4.0,
            max_near: 1.0,
            max_far: 1.0,
            min_blur: 0.0,
            kernel: 16.0,
            grain: 0.0,
            desaturation: 0.0,
            shadows: Vec3::ZERO,
            highlights: Vec3::ONE,
            midtones: Vec3::ONE,
            colorize: Vec3::ONE,
            blend: 1.0,
        }
    }
}

impl PostSettings {
    /// From a PostProcessSettings struct. Fields it doesn't store are the struct defaults, except
    /// those whose bOverride_ flag defaults to false (DOF_MinBlurAmount, Scene_Colorize,
    /// Scene_ImageGrainScale): those keep `base` (the level's settings) unless the flag is set.
    pub fn from_props(s: &[upk::props::Prop], base: Option<&PostSettings>) -> Self {
        use upk::props::{find, Value};
        let d = PostSettings::default();
        let f = |k: &str, def: f32| match find(s, k) {
            Some(Value::Float(v)) => *v,
            _ => def,
        };
        let v = |k: &str, def: Vec3| match find(s, k) {
            Some(Value::Vector(x)) => Vec3::from(*x),
            _ => def,
        };
        let on = |k: &str| matches!(find(s, k), Some(Value::Bool(true)));
        let kept = |flag: &str, own: f32, b: f32| if base.is_none() || on(flag) { own } else { b };
        let b = base.cloned().unwrap_or_default();
        PostSettings {
            dof: on("bEnableDOF"),
            focus_distance: f("DOF_FocusDistance", d.focus_distance),
            focus_radius: f("DOF_FocusInnerRadius", d.focus_radius),
            falloff: f("DOF_FalloffExponent", d.falloff),
            max_near: f("DOF_MaxNearBlurAmount", d.max_near),
            max_far: f("DOF_MaxFarBlurAmount", d.max_far),
            min_blur: kept("bOverride_DOF_MinBlurAmount", f("DOF_MinBlurAmount", d.min_blur), b.min_blur),
            kernel: f("DOF_BlurKernelSize", d.kernel),
            grain: kept("bOverride_Scene_ImageGrainScale", f("Scene_ImageGrainScale", d.grain), b.grain),
            desaturation: f("Scene_Desaturation", d.desaturation),
            shadows: v("Scene_Shadows", d.shadows),
            highlights: v("Scene_HighLights", d.highlights),
            midtones: v("Scene_MidTones", d.midtones),
            colorize: if base.is_none() || on("bOverride_Scene_Colorize") { v("Scene_Colorize", d.colorize) } else { b.colorize },
            blend: f("DOF_InterpolationDuration", d.blend),
        }
    }

    /// Move towards `to` by `k` (0..1).
    pub fn lerp(&self, to: &PostSettings, k: f32) -> PostSettings {
        let l = |a: f32, b: f32| a + (b - a) * k;
        let lv = |a: Vec3, b: Vec3| a.lerp(b, k);
        PostSettings {
            dof: if k >= 1.0 { to.dof } else { self.dof || to.dof },
            focus_distance: l(self.focus_distance, to.focus_distance),
            focus_radius: l(self.focus_radius, to.focus_radius),
            falloff: l(self.falloff, to.falloff),
            max_near: l(self.max_near, to.max_near),
            max_far: l(self.max_far, to.max_far),
            min_blur: l(self.min_blur, to.min_blur),
            kernel: l(self.eff_kernel(), to.eff_kernel()),
            grain: l(self.grain, to.grain),
            desaturation: l(self.desaturation, to.desaturation),
            shadows: lv(self.shadows, to.shadows),
            highlights: lv(self.highlights, to.highlights),
            midtones: lv(self.midtones, to.midtones),
            colorize: lv(self.colorize, to.colorize),
            blend: to.blend,
        }
    }

    fn eff_kernel(&self) -> f32 {
        if self.dof { self.kernel } else { 0.0 }
    }

    /// The shader's uniform (distances to metres).
    pub fn uniform(&self, time: f32) -> UberPost {
        let m = crate::convert::S;
        let kernel = std::env::var("GOAT_DOF_KERNEL").ok().and_then(|v| v.parse().ok()).filter(|_| self.dof).unwrap_or(self.eff_kernel());
        UberPost {
            focus_distance: self.focus_distance * m,
            focus_radius: self.focus_radius * m,
            falloff: self.falloff,
            max_near: self.max_near,
            max_far: self.max_far,
            min_blur: self.min_blur,
            kernel,
            grain: self.grain,
            near_plane: 0.05,
            time,
            desaturation: self.desaturation,
            shadows: self.shadows.extend(0.0),
            highlights: self.highlights.extend(1.0),
            midtones: self.midtones.extend(1.0),
            colorize: self.colorize.extend(1.0),
        }
    }
}

pub struct PostPlugin;

impl Plugin for PostPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "post.wgsl");
        app.add_plugins((ExtractComponentPlugin::<UberPost>::default(), UniformComponentPlugin::<UberPost>::default()))
            .add_systems(Update, tick);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        render_app
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_systems(Core3d, post_pass.in_set(Core3dSystems::PostProcess).before(tonemapping));
    }
}

fn tick(time: Res<Time>, mut q: Query<&mut UberPost>) {
    for mut u in &mut q {
        u.time = time.elapsed_secs() % 1000.0;
    }
}

#[derive(Resource)]
struct PostPipeline {
    /// (single-sampled depth, multisampled depth) layouts.
    layouts: [BindGroupLayoutDescriptor; 2],
    sampler: Sampler,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
    cache: std::collections::HashMap<(TextureFormat, u32), CachedRenderPipelineId>,
}

fn init_pipeline(mut commands: Commands, render_device: Res<RenderDevice>, asset_server: Res<AssetServer>, fullscreen: Res<FullscreenShader>) {
    let layout = |ms: bool| {
        BindGroupLayoutDescriptor::new(
            "goat_post_layout",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::FRAGMENT,
                (
                    texture_2d(TextureSampleType::Float { filterable: true }),
                    sampler(SamplerBindingType::Filtering),
                    uniform_buffer::<UberPost>(true),
                    if ms { texture_depth_2d_multisampled() } else { texture_depth_2d() },
                ),
            ),
        )
    };
    commands.insert_resource(PostPipeline {
        layouts: [layout(false), layout(true)],
        sampler: render_device.create_sampler(&SamplerDescriptor::default()),
        shader: asset_server.load("embedded://goat/post.wgsl"),
        fullscreen: fullscreen.clone(),
        cache: default(),
    });
}

#[derive(Component)]
struct PostPipelineId(CachedRenderPipelineId, bool);

fn prepare_pipelines(mut commands: Commands, cache: Res<PipelineCache>, mut pipeline: ResMut<PostPipeline>, views: Query<(Entity, &ExtractedView, &Msaa, Option<&UberPost>), With<ExtractedCamera>>) {
    for (e, view, msaa, post) in &views {
        if post.is_none() {
            commands.entity(e).remove::<PostPipelineId>();
            continue;
        }
        let samples = msaa.samples();
        let key = (view.target_format, samples);
        let ms = samples > 1;
        let id = match pipeline.cache.get(&key) {
            Some(id) => *id,
            None => {
                let mut defs: Vec<ShaderDefVal> = vec![];
                if ms {
                    defs.push("MULTISAMPLED".into());
                }
                let id = cache.queue_render_pipeline(RenderPipelineDescriptor {
                    label: Some("goat_post_pipeline".into()),
                    layout: vec![pipeline.layouts[ms as usize].clone()],
                    vertex: pipeline.fullscreen.to_vertex_state(),
                    fragment: Some(FragmentState {
                        shader: pipeline.shader.clone(),
                        shader_defs: defs,
                        targets: vec![Some(ColorTargetState { format: view.target_format, blend: None, write_mask: ColorWrites::ALL })],
                        ..default()
                    }),
                    ..default()
                });
                pipeline.cache.insert(key, id);
                id
            }
        };
        commands.entity(e).insert(PostPipelineId(id, ms));
    }
}

fn post_pass(
    view: ViewQuery<(&ViewTarget, &ViewDepthTexture, &DynamicUniformIndex<UberPost>, &PostPipelineId)>,
    pipeline: Res<PostPipeline>,
    cache: Res<PipelineCache>,
    uniforms: Res<ComponentUniforms<UberPost>>,
    render_device: Res<RenderDevice>,
    mut ctx: RenderContext,
) {
    let (target, depth, index, pid) = view.into_inner();
    let Some(rp) = cache.get_render_pipeline(pid.0) else { return };
    let Some(binding) = uniforms.uniforms().binding() else { return };
    let post = target.post_process_write();
    let bind_group = render_device.create_bind_group(
        "goat_post_bind_group",
        &cache.get_bind_group_layout(&pipeline.layouts[pid.1 as usize]),
        &BindGroupEntries::sequential((post.source, &pipeline.sampler, binding, depth.view())),
    );
    let desc = RenderPassDescriptor {
        label: Some("goat_post_pass"),
        color_attachments: &[Some(RenderPassColorAttachment { view: post.destination, depth_slice: None, resolve_target: None, ops: Operations::default() })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    };
    let mut pass = ctx.command_encoder().begin_render_pass(&desc);
    pass.set_pipeline(rp);
    pass.set_bind_group(0, &bind_group, &[index.index()]);
    pass.draw(0..3, 0..1);
}
