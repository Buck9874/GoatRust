//! UE3 material graphs evaluated on the CPU: DiffuseColor graphs beyond one plain texture
//! (multiplied detail textures, channel masks, tints; upk::material::diffuse_graph) baked into
//! one texture in linear space, and OpacityMask channels copied into alpha.

use std::collections::HashMap;
use upk::material::Node;
use upk::registry::ObjRef;
use upk::texture::Image;

/// A graph worth baking: it samples a texture and is more than that texture as it is.
pub fn needs_bake(n: &Node) -> bool {
    let mut t = Vec::new();
    n.textures(&mut t);
    !t.is_empty() && !matches!(n, Node::Tex { tiling, .. } if *tiling == [1.0, 1.0])
}

fn srgb_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        })
    })
}

fn eval(n: &Node, u: f32, v: f32, texs: &HashMap<ObjRef, (Image, bool)>, lut: &[f32; 256]) -> [f32; 4] {
    match n {
        Node::Tex { tex, tiling } => {
            let (img, srgb) = &texs[tex];
            let x = ((u * tiling[0]).rem_euclid(1.0) * img.width as f32) as u32 % img.width;
            let y = ((v * tiling[1]).rem_euclid(1.0) * img.height as f32) as u32 % img.height;
            let p = &img.rgba[((y * img.width + x) * 4) as usize..][..4];
            let c = |k: usize| if *srgb { lut[p[k] as usize] } else { p[k] as f32 / 255.0 };
            [c(0), c(1), c(2), p[3] as f32 / 255.0]
        }
        Node::Const(c) => *c,
        Node::Channel(a, k) => {
            let x = eval(a, u, v, texs, lut)[*k];
            [x, x, x, x]
        }
        Node::Mul(a, b) => {
            let (a, b) = (eval(a, u, v, texs, lut), eval(b, u, v, texs, lut));
            [a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]]
        }
        Node::Add(a, b) => {
            let (a, b) = (eval(a, u, v, texs, lut), eval(b, u, v, texs, lut));
            [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
        }
        Node::OneMinus(a) => eval(a, u, v, texs, lut).map(|x| 1.0 - x),
        Node::Lerp(a, b, t) => {
            let (a, b, t) = (eval(a, u, v, texs, lut), eval(b, u, v, texs, lut), eval(t, u, v, texs, lut)[0]);
            [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t)
        }
    }
}

/// Evaluate a graph per texel at its largest texture's size; sRGB-encoded RGB, alpha from the
/// result (opaque when no texture has alpha).
pub fn bake(n: &Node, texs: &HashMap<ObjRef, (Image, bool)>) -> Option<Image> {
    let mut refs = Vec::new();
    n.textures(&mut refs);
    if refs.iter().any(|t| !texs.contains_key(t)) {
        return None;
    }
    let (w, h) = refs.iter().map(|t| (texs[t].0.width, texs[t].0.height)).max_by_key(|(w, h)| w * h)?;
    let lut = srgb_lut();
    let enc = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        let s = if x <= 0.0031308 { x * 12.92 } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0 + 0.5) as u8
    };
    let mut rgba = vec![255u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let c = eval(n, (x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32, texs, lut);
            let i = ((y * w + x) * 4) as usize;
            rgba[i] = enc(c[0]);
            rgba[i + 1] = enc(c[1]);
            rgba[i + 2] = enc(c[2]);
            rgba[i + 3] = (c[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    Some(Image { width: w, height: h, rgba })
}

/// BLEND_Masked: alpha = the mask texture's channel (nearest), inverted for a OneMinus.
pub fn apply_mask(img: &mut Image, mask: &Image, ch: usize, invert: bool) {
    let (w, h) = (img.width, img.height);
    for y in 0..h {
        for x in 0..w {
            let mx = (x as u64 * mask.width as u64 / w as u64) as u32;
            let my = (y as u64 * mask.height as u64 / h as u64) as u32;
            let v = mask.rgba[((my * mask.width + mx) * 4) as usize + ch];
            img.rgba[((y * w + x) * 4 + 3) as usize] = if invert { 255 - v } else { v };
        }
    }
}
