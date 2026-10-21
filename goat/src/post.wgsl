// UE3 depth of field (DOFAndBloomCommon.usf CalcUnfocusedPercent) and image grain.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct UberPost {
    focus_distance: f32,
    focus_radius: f32,
    falloff: f32,
    max_near: f32,
    max_far: f32,
    min_blur: f32,
    kernel: f32,
    grain: f32,
    near_plane: f32,
    time: f32,
    desaturation: f32,
    shadows: vec4<f32>,
    highlights: vec4<f32>,
    midtones: vec4<f32>,
    colorize: vec4<f32>,
}

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> s: UberPost;
#ifdef MULTISAMPLED
@group(0) @binding(3) var depth: texture_depth_multisampled_2d;
#else
@group(0) @binding(3) var depth: texture_depth_2d;
#endif

// view distance (m) at a pixel: Bevy's infinite reverse-Z depth = near / z
fn scene_depth(p: vec2<i32>) -> f32 {
    let d = textureLoad(depth, p, 0);
    return s.near_plane / max(d, 1e-7);
}

fn unfocused(z: f32) -> f32 {
    let rel = z - s.focus_distance;
    let max_blur = select(s.max_far, s.max_near, rel < 0.0);
    let p = pow(saturate(abs(rel) / max(s.focus_radius, 1e-3)), s.falloff);
    return min(max_blur, max(s.min_blur, p));
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(screen));
    var color = textureSample(screen, samp, in.uv);
    if (s.kernel > 0.0) {
        let px = vec2<i32>(in.uv * size);
        let r = unfocused(scene_depth(px)) * s.kernel * size.y / 720.0;
        if (r > 0.25) {
            // a disc of 8 taps plus the centre
            var sum = color.rgb;
            for (var i = 0; i < 8; i++) {
                let a = f32(i) * 0.7853982;
                sum += textureSample(screen, samp, in.uv + vec2<f32>(cos(a), sin(a)) * r / size).rgb;
            }
            color = vec4<f32>(sum / 9.0, color.a);
        }
    }
    // scene tone (UE3 uber post: shadows, highlights, midtones, desaturation, colorize)
    var c = max(color.rgb - s.shadows.rgb, vec3<f32>(0.0)) * s.highlights.rgb;
    c = pow(c, s.midtones.rgb);
    let grey = dot(c, vec3<f32>(0.3, 0.59, 0.11));
    c = mix(c, vec3<f32>(grey), s.desaturation) * s.colorize.rgb;
    color = vec4<f32>(c, color.a);
    if (s.grain > 0.0) {
        let n = hash(in.uv * size + vec2<f32>(s.time * 61.0, s.time * 37.0)) - 0.5;
        color = vec4<f32>(color.rgb * (1.0 + n * 2.0 * s.grain), color.a);
    }
    return color;
}
