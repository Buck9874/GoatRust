//! Baked lighting: UE3 directional lightmaps (FLightMap2D) turned into Bevy lightmaps.
//!
//! UE3's high-quality lightmap shader (decoded from the cooked DXBC) lights a surface with
//! `DiffuseColor * (T0 * LightMapScale[0]) * dot(T1 * LightMapScale[1], 1/3)`, where T0 is the
//! NormalizedAverageColor atlas and T1 the DirectionalMaxComponent atlas (both DXT1, sRGB) and
//! the scales are per component. Components share atlases but each owns a rectangle of it
//! (UV = lightmap UV * CoordinateScale + CoordinateBias), so the scales are baked into one HDR
//! RGB atlas per pair, and the atlases are packed into a few big pages for batching.
//!
//! The dominant directional light (the sun) is not in the lightmaps: UE3 lights static surfaces
//! with it per pixel, shadowed by each component's ShadowMap2D (a G8 distance field, UV =
//! lightmap UV * its own CoordinateScale + CoordinateBias). Those are baked next to the lightmap
//! texels and stored, half resolution, in each page's mip 1; the patched bevy_pbr shader
//! (lightmap_shadow) shadows the first directional light with min(realtime, baked).

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::Lightmap;
use bevy::math::{Rect, UVec2, Vec2};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use upk::level::LightMap2D;
use upk::package::Package;

/// Page size of the packed lightmap atlases.
const PAGE: u32 = 4096;
/// Texels of clamped border around each source atlas on a page.
const GUTTER: u32 = 2;

/// Lightmap brightness: a lightmap value of 1 lights a surface as much as a UE3 light of
/// Brightness 1 (the sun uses the same scale, see `sun_illuminance`).
pub fn exposure() -> f32 {
    1.0 / bevy::camera::Exposure::default().exposure()
}

/// Bevy illuminance of a UE3 directional light of `brightness` (UE3 lights a surface with
/// brightness * N.L, Bevy with illuminance * exposure * N.L / PI).
pub fn sun_illuminance(brightness: f32) -> f32 {
    brightness * std::f32::consts::PI * exposure()
}

/// Baked lightmap pages and where each source atlas went.
#[derive(Default)]
pub struct Lightmaps {
    pages: Vec<(Handle<Image>, UVec2)>,
    /// (T0, T1) -> (page, texel offset, atlas size)
    place: HashMap<(i32, i32), (usize, UVec2, UVec2)>,
    pub texels: u64,
}

impl Lightmaps {
    /// The Bevy lightmap of a component (the mesh needs its lightmap UVs in UV_1).
    pub fn get(&self, lm: &LightMap2D) -> Option<Lightmap> {
        let &(page, off, size) = self.place.get(&(lm.textures[0], lm.textures[1]))?;
        let (image, dims) = &self.pages[page];
        let to_page = |uv: Vec2| (off.as_vec2() + uv * size.as_vec2()) / dims.as_vec2();
        let bias = Vec2::from(lm.coord_bias);
        let scale = Vec2::from(lm.coord_scale);
        Some(Lightmap { image: image.clone(), uv_rect: Rect { min: to_page(bias), max: to_page(bias + scale) }, bicubic_sampling: false })
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }
}

fn srgb_to_linear(c: u8) -> f32 {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
    })[c as usize]
}

/// Pack linear RGB into RGB9E5 (shared exponent).
pub fn rgb9e5(c: [f32; 3]) -> u32 {
    const N: i32 = 9;
    const B: i32 = 15;
    let max_val = ((1 << N) - 1) as f32 / (1 << N) as f32 * 2f32.powi(31 - B);
    let [r, g, b] = c.map(|v| if v.is_finite() { v.clamp(0.0, max_val) } else { 0.0 });
    let maxc = r.max(g).max(b);
    if maxc <= 1e-9 {
        return 0;
    }
    let mut exp = (maxc.log2().floor() as i32).max(-B - 1) + 1 + B;
    let mut denom = 2f32.powi(exp - B - N);
    if (maxc / denom + 0.5).floor() as i32 == 1 << N {
        denom *= 2.0;
        exp += 1;
    }
    let m = |v: f32| ((v / denom + 0.5).floor() as u32).min((1 << N) - 1);
    m(r) | (m(g) << 9) | (m(b) << 18) | ((exp as u32) << 27)
}

/// One source atlas baked to linear RGB with its components' scales applied.
struct Baked {
    key: (i32, i32),
    w: u32,
    h: u32,
    rgb: Vec<[f32; 3]>,
    /// Sun shadow factor per texel (1 = lit).
    shadow: Vec<f32>,
    /// Share of each HL2 basis direction in the directional lightmap (w0, w1; w2 = 1 - both).
    wts: Vec<[f32; 2]>,
}

/// A decoded ShadowMap2D: (texture, CoordinateScale, CoordinateBias, bIsShadowFactorTexture).
struct ShadowSrc {
    img: upk::texture::Image,
    scale: [f32; 2],
    bias: [f32; 2],
    factor: bool,
}

impl ShadowSrc {
    fn load(p: &Package, obj: i32) -> Option<Self> {
        if obj <= 0 {
            return None;
        }
        let (_, ps) = upk::props::export_props(p, obj as usize - 1).ok()?;
        let tex = match upk::props::find(&ps, "Texture") {
            Some(upk::props::Value::Object(o)) if *o > 0 => *o,
            _ => return None,
        };
        let v2 = |k: &str, d: [f32; 2]| match upk::props::find(&ps, k) {
            Some(upk::props::Value::Vector2D(v)) => *v,
            _ => d,
        };
        let factor = matches!(upk::props::find(&ps, "bIsShadowFactorTexture"), Some(upk::props::Value::Bool(true)));
        let t = upk::texture::Texture::parse(p, tex as usize - 1).ok()?;
        let img = t.decode(p, tex as usize - 1, i32::MAX).ok()?;
        Some(Self { img, scale: v2("CoordinateScale", [1.0, 1.0]), bias: v2("CoordinateBias", [0.0, 0.0]), factor })
    }

    /// Bilinear sample at a shadow map UV -> shadow factor.
    fn sample(&self, uv: [f32; 2]) -> f32 {
        let (w, h) = (self.img.width as f32, self.img.height as f32);
        let x = (uv[0] * w - 0.5).clamp(0.0, w - 1.0);
        let y = (uv[1] * h - 0.5).clamp(0.0, h - 1.0);
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.img.width - 1), (y0 + 1).min(self.img.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: u32, y: u32| self.img.rgba[((y * self.img.width + x) * 4) as usize] as f32 / 255.0;
        let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
        let bot = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
        let d = top * (1.0 - fy) + bot * fy;
        if self.factor {
            d
        } else {
            // a distance field: 0.5 is the shadow edge (a narrow penumbra around it)
            let t = ((d - 0.45) / 0.1).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
    }
}

fn bake_atlas(p: &Package, key: (i32, i32), comps: &[LightMap2D]) -> Option<Baked> {
    let decode = |obj: i32| -> Option<upk::texture::Image> {
        let t = upk::texture::Texture::parse(p, obj as usize - 1).ok()?;
        t.decode(p, obj as usize - 1, i32::MAX).ok()
    };
    let t0 = decode(key.0)?;
    let t1 = decode(key.1)?;
    if (t0.width, t0.height) != (t1.width, t1.height) {
        return None;
    }
    let (w, h) = (t0.width, t0.height);
    let n = (w * h) as usize;
    let mut rgb = vec![[0f32; 3]; n];
    let mut shadow = vec![1f32; n];
    let mut wts = vec![[1.0f32 / 3.0; 2]; n];
    let mut set = vec![false; n];
    let mut shadow_cache: HashMap<i32, Option<ShadowSrc>> = HashMap::new();
    for lm in comps {
        let sm = shadow_cache.entry(lm.shadow).or_insert_with(|| ShadowSrc::load(p, lm.shadow));
        let (s0, s1) = (lm.scales[0], lm.scales[1]);
        let x0 = (lm.coord_bias[0] * w as f32).floor().max(0.0) as u32;
        let y0 = (lm.coord_bias[1] * h as f32).floor().max(0.0) as u32;
        let x1 = (((lm.coord_bias[0] + lm.coord_scale[0]) * w as f32).ceil() as u32).min(w);
        let y1 = (((lm.coord_bias[1] + lm.coord_scale[1]) * h as f32).ceil() as u32).min(h);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * w + x) as usize;
                let a = &t0.rgba[i * 4..i * 4 + 3];
                let d = &t1.rgba[i * 4..i * 4 + 3];
                let b = [srgb_to_linear(d[0]) * s1[0], srgb_to_linear(d[1]) * s1[1], srgb_to_linear(d[2]) * s1[2]];
                let dir = (b[0] + b[1] + b[2]) / 3.0;
                if dir > 1e-6 {
                    wts[i] = [b[0] / (dir * 3.0), b[1] / (dir * 3.0)];
                }
                rgb[i] = [srgb_to_linear(a[0]) * s0[0] * dir, srgb_to_linear(a[1]) * s0[1] * dir, srgb_to_linear(a[2]) * s0[2] * dir];
                if let Some(sm) = sm.as_ref() {
                    // atlas texel -> the component's lightmap UV -> its shadow map UV
                    let u = ((x as f32 + 0.5) / w as f32 - lm.coord_bias[0]) / lm.coord_scale[0].max(1e-6);
                    let v = ((y as f32 + 0.5) / h as f32 - lm.coord_bias[1]) / lm.coord_scale[1].max(1e-6);
                    shadow[i] = sm.sample([u * sm.scale[0] + sm.bias[0], v * sm.scale[1] + sm.bias[1]]);
                }
                set[i] = true;
            }
        }
    }
    // grow the lit texels into the unused ones so bilinear filtering doesn't pull in black
    for _ in 0..2 {
        let prev = set.clone();
        let src = rgb.clone();
        let src_shadow = shadow.clone();
        let src_wts = wts.clone();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let i = (y as u32 * w + x as u32) as usize;
                if prev[i] {
                    continue;
                }
                let (mut sum, mut sum_s, mut sum_w, mut k) = ([0f32; 3], 0f32, [0f32; 2], 0);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                            continue;
                        }
                        let j = (ny as u32 * w + nx as u32) as usize;
                        if prev[j] {
                            for c in 0..3 {
                                sum[c] += src[j][c];
                            }
                            sum_s += src_shadow[j];
                            sum_w[0] += src_wts[j][0];
                            sum_w[1] += src_wts[j][1];
                            k += 1;
                        }
                    }
                }
                if k > 0 {
                    rgb[i] = sum.map(|v| v / k as f32);
                    shadow[i] = sum_s / k as f32;
                    wts[i] = [sum_w[0] / k as f32, sum_w[1] / k as f32];
                    set[i] = true;
                }
            }
        }
    }
    Some(Baked { key, w, h, rgb, shadow, wts })
}

/// A page: mip 0 the lightmap, mip 1 the sun shadow factor (grey, half resolution).
fn page_image(w: u32, h: u32, data: Vec<u32>, shadow: &[f32], wts: &[[f32; 2]]) -> Image {
    let mut image = Image::new_uninit(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, TextureFormat::Rgb9e5Ufloat, RenderAssetUsages::RENDER_WORLD);
    let (hw, hh) = (w / 2, h / 2);
    let mut bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
    if !shadow.is_empty() {
        image.texture_descriptor.mip_level_count = 2;
        for y in 0..hh {
            for x in 0..hw {
                let idx = |dx: u32, dy: u32| ((y * 2 + dy) * w + x * 2 + dx) as usize;
                let at = |dx: u32, dy: u32| shadow[idx(dx, dy)];
                let s = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) * 0.25;
                let wt = |c: usize| wts.get(idx(0, 0)).map(|_| (wts[idx(0, 0)][c] + wts[idx(1, 0)][c] + wts[idx(0, 1)][c] + wts[idx(1, 1)][c]) * 0.25).unwrap_or(1.0 / 3.0);
                // mip 1: (sun shadow, basis weight 0, basis weight 1)
                bytes.extend_from_slice(&rgb9e5([s, wt(0), wt(1)]).to_le_bytes());
            }
        }
    }
    image.data = Some(bytes);
    let mut s = ImageSamplerDescriptor::linear();
    s.address_mode_u = ImageAddressMode::ClampToEdge;
    s.address_mode_v = ImageAddressMode::ClampToEdge;
    image.sampler = ImageSampler::Descriptor(s);
    image
}

/// Bake every lightmap the components use into packed pages.
pub fn bake(p: &Package, lightmaps: &[LightMap2D], images: &mut Assets<Image>) -> Lightmaps {
    let mut by_atlas: HashMap<(i32, i32), Vec<LightMap2D>> = HashMap::new();
    for lm in lightmaps {
        let v = by_atlas.entry((lm.textures[0], lm.textures[1])).or_default();
        if !v.contains(lm) {
            v.push(*lm);
        }
    }
    let jobs: Vec<((i32, i32), Vec<LightMap2D>)> = by_atlas.into_iter().collect();
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(16);
    // atlases differ a lot in size: hand them out one at a time, biggest group first
    let mut jobs = jobs;
    jobs.sort_by_key(|(_, c)| std::cmp::Reverse(c.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut baked: Vec<Baked> = Vec::new();
    std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut out = vec![];
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((k, c)) = jobs.get(i) else { break };
                        out.extend(bake_atlas(p, *k, c));
                    }
                    out
                })
            })
            .collect();
        for h in handles {
            baked.extend(h.join().unwrap_or_default());
        }
    });
    let t_pack = std::time::Instant::now();
    // shelf-pack, tallest first
    baked.sort_by(|a, b| b.h.cmp(&a.h).then(b.w.cmp(&a.w)).then(a.key.cmp(&b.key)));
    let mut out = Lightmaps::default();
    let mut page: Vec<u32> = Vec::new();
    let mut page_shadow: Vec<f32> = Vec::new();
    let mut page_wts: Vec<[f32; 2]> = Vec::new();
    let with_shadows = std::env::var_os("GOAT_NO_BAKED_SHADOWS").is_none();
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    let mut used_h = 0u32;
    let mut flush = |page: &mut Vec<u32>, page_shadow: &mut Vec<f32>, page_wts: &mut Vec<[f32; 2]>, used_h: u32, out: &mut Lightmaps| {
        if page.is_empty() {
            return;
        }
        let h = used_h.next_multiple_of(4).clamp(4, PAGE);
        page.truncate((PAGE * h) as usize);
        page_shadow.truncate((PAGE * h) as usize);
        page_wts.truncate((PAGE * h) as usize);
        let shadow = if with_shadows { std::mem::take(page_shadow) } else { vec![] };
        let wts = std::mem::take(page_wts);
        out.pages.push((images.add(page_image(PAGE, h, std::mem::take(page), &shadow, &wts)), UVec2::new(PAGE, h)));
    };
    for b in &baked {
        let (bw, bh) = (b.w + 2 * GUTTER, b.h + 2 * GUTTER);
        if bw > PAGE || bh > PAGE {
            continue;
        }
        if x + bw > PAGE {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        if y + bh > PAGE || page.is_empty() {
            if !page.is_empty() {
                flush(&mut page, &mut page_shadow, &mut page_wts, used_h, &mut out);
            }
            page = vec![0u32; (PAGE * PAGE) as usize];
            page_shadow = vec![1f32; (PAGE * PAGE) as usize];
            page_wts = vec![[1.0 / 3.0; 2]; (PAGE * PAGE) as usize];
            (x, y, shelf, used_h) = (0, 0, 0, 0);
        }
        // copy with the edge texels clamped out into the gutter
        for py in 0..bh {
            let sy = (py as i32 - GUTTER as i32).clamp(0, b.h as i32 - 1) as u32;
            for px in 0..bw {
                let sx = (px as i32 - GUTTER as i32).clamp(0, b.w as i32 - 1) as u32;
                page[((y + py) * PAGE + x + px) as usize] = rgb9e5(b.rgb[(sy * b.w + sx) as usize]);
                page_shadow[((y + py) * PAGE + x + px) as usize] = b.shadow[(sy * b.w + sx) as usize];
                page_wts[((y + py) * PAGE + x + px) as usize] = b.wts[(sy * b.w + sx) as usize];
            }
        }
        out.place.insert(b.key, (out.pages.len(), UVec2::new(x + GUTTER, y + GUTTER), UVec2::new(b.w, b.h)));
        out.texels += (b.w * b.h) as u64;
        x += bw;
        shelf = shelf.max(bh);
        used_h = used_h.max(y + bh);
    }
    flush(&mut page, &mut page_shadow, &mut page_wts, used_h, &mut out);
    bevy::log::debug!("lightmap pack in {:?}", t_pack.elapsed());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack(v: u32) -> [f32; 3] {
        let e = (v >> 27) as i32 - 15 - 9;
        let s = 2f32.powi(e);
        [(v & 511) as f32 * s, ((v >> 9) & 511) as f32 * s, ((v >> 18) & 511) as f32 * s]
    }

    #[test]
    fn rgb9e5_round_trips() {
        for c in [[0.0, 0.0, 0.0], [1.0, 0.5, 0.25], [12.0, 3.0, 0.01], [0.002, 0.004, 0.003]] {
            let d = unpack(rgb9e5(c));
            let m = c[0].max(c[1]).max(c[2]);
            for k in 0..3 {
                assert!((d[k] - c[k]).abs() <= m / 256.0 + 1e-6, "{c:?} -> {d:?}");
            }
        }
    }
}
