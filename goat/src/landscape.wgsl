// Landscape paint-layer blend: four tiled layer textures weighted by per-vertex weights
// (vertex colour channels = the landscape's weightmap channels).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
}
#endif

struct LandscapeParams {
    tiling: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> params: LandscapeParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var layer0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var layer0_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var layer1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var layer1_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var layer2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var layer2_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var layer3: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var layer3_sampler: sampler;

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

#ifdef VERTEX_COLORS
    let w = in.color;
#else
    let w = vec4<f32>(0.0, 0.0, 1.0, 0.0);
#endif
#ifdef VERTEX_UVS_A
    let uv = in.uv;
#else
    let uv = vec2<f32>(0.0, 0.0);
#endif
    let c0 = textureSample(layer0, layer0_sampler, uv * params.tiling.x);
    let c1 = textureSample(layer1, layer1_sampler, uv * params.tiling.y);
    let c2 = textureSample(layer2, layer2_sampler, uv * params.tiling.z);
    let c3 = textureSample(layer3, layer3_sampler, uv * params.tiling.w);
    let sum = max(w.x + w.y + w.z + w.w, 0.001);
    let blended = (c0 * w.x + c1 * w.y + c2 * w.z + c3 * w.w) / sum;
    pbr_input.material.base_color = vec4<f32>(blended.rgb, 1.0);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

#ifdef PREPASS_PIPELINE
    let out = deferred_output(in, pbr_input);
#else
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif
    return out;
}
