//! The map's post-process look: WorldInfo.DefaultPostProcessSettings' colour grading LUT
//! (ColorLookup.Textures.level_01: a 256x16 sRGB-encoded 16^3 table) fitted to Bevy's colour
//! grading (mid-tone contrast and saturation), bloom as the settings say (Goatville: off), and
//! the level's HeightFog (UE3: fog = 1 - exp(-Density x distance) under Height, LightColor) as
//! Bevy exponential distance fog.

use bevy::prelude::*;
use bevy::render::view::{ColorGrading, ColorGradingGlobal, ColorGradingSection};
use upk::registry::{ObjRef, Registry};

#[derive(Resource, Clone, Debug, Default)]
pub struct LevelLook {
    /// (mid-tone contrast, saturation) fitted to the LUT.
    pub grading: Option<(f32, f32)>,
    /// HeightFog: density per metre, colour.
    pub fog: Option<(f32, Color)>,
    /// The level's post-process settings (WorldInfo.DefaultPostProcessSettings, post.rs) and
    /// each PostProcessVolume's, by volume name.
    pub post: Option<crate::post::PostSettings>,
    pub volume_post: std::collections::HashMap<String, crate::post::PostSettings>,
    applied: bool,
}

/// Fit contrast (slope of the grey ramp around the middle) and saturation (chroma ratio) to a
/// 256x16 UE3 colour LUT.
pub fn fit_lut(img: &upk::texture::Image) -> Option<(f32, f32)> {
    if img.width != 256 || img.height != 16 {
        return None;
    }
    let n = 16u32;
    let at = |r: u32, g: u32, b: u32| -> [f32; 3] {
        let i = ((g * img.width + b * n + r) * 4) as usize;
        [img.rgba[i] as f32, img.rgba[i + 1] as f32, img.rgba[i + 2] as f32]
    };
    let lum = |c: [f32; 3]| (c[0] + c[1] + c[2]) / 3.0;
    // grey ramp slope between 1/3 and 2/3 (encoded values)
    let (lo, hi) = (lum(at(5, 5, 5)), lum(at(10, 10, 10)));
    let contrast = ((hi - lo) / (85.0)).clamp(0.5, 2.0);
    let mut ratios = vec![];
    for r in (0..16).step_by(3) {
        for g in (0..16).step_by(3) {
            for b in (0..16).step_by(3) {
                let i = [r as f32 * 17.0, g as f32 * 17.0, b as f32 * 17.0];
                let o = at(r, g, b);
                let ci = i.iter().cloned().fold(f32::MIN, f32::max) - i.iter().cloned().fold(f32::MAX, f32::min);
                let co = o.iter().cloned().fold(f32::MIN, f32::max) - o.iter().cloned().fold(f32::MAX, f32::min);
                if ci > 60.0 {
                    ratios.push(co / ci);
                }
            }
        }
    }
    let sat = (ratios.iter().sum::<f32>() / ratios.len().max(1) as f32).clamp(0.5, 2.0);
    Some((contrast, sat))
}

/// Read the look from the map (package 0 of the registry).
pub fn load(reg: &Registry) -> LevelLook {
    let mut out = LevelLook::default();
    let map = &reg.packages[0];
    let mut best_height = f32::MIN;
    let mut volume_settings: Vec<(String, Vec<upk::props::Prop>)> = vec![];
    for i in 0..map.exports.len() {
        let class = map.class_name(i as i32 + 1);
        let path = map.obj_path(i as i32 + 1);
        if !path.contains(".PersistentLevel.") {
            continue;
        }
        let r = ObjRef { pkg: 0, export: i };
        if class == "WorldInfo" && out.grading.is_none() {
            let ps = reg.merged_props(r);
            let Some((pk, upk::props::Value::Struct(_, s))) = upk::registry::find(&ps, "DefaultPostProcessSettings") else { continue };
            let lut = match upk::props::find(s, "ColorGrading_LookupTable") {
                Some(upk::props::Value::Object(o)) => reg.resolve(pk, *o),
                _ => None,
            };
            let base = crate::post::PostSettings::from_props(s, None);
            info!("post: {base:?}");
            out.post = Some(base);
            if let Some(t) = lut {
                if let Some(img) = upk::texture::Texture::parse(reg.pkg(t), t.export).ok().and_then(|tx| tx.decode(reg.pkg(t), t.export, 4096).ok()) {
                    out.grading = fit_lut(&img);
                    info!("colour grading LUT {}: contrast {:.2}, saturation {:.2}", reg.path(t), out.grading.map(|g| g.0).unwrap_or(1.0), out.grading.map(|g| g.1).unwrap_or(1.0));
                }
            }
        }
        if class == "PostProcessVolume" {
            let ps = reg.merged_props(r);
            if let Some((_, upk::props::Value::Struct(_, st))) = upk::registry::find(&ps, "Settings") {
                volume_settings.push((map.obj_name(i as i32 + 1), st.clone()));
            }
        }
        if class == "HeightFogComponent" {
            let ps = reg.merged_props(r);
            // the fog that covers the whole level: the one with the highest ceiling (others sit low,
            // e.g. under the map)
            let owner = map.exports[i].outer;
            let height = if owner > 0 {
                match upk::registry::find(&reg.merged_props(ObjRef { pkg: 0, export: owner as usize - 1 }), "Height") {
                    Some((_, upk::props::Value::Float(h))) => *h,
                    _ => f32::MIN,
                }
            } else {
                f32::MIN
            };
            if height < best_height {
                continue;
            }
            if matches!(upk::registry::find(&ps, "bEnabled"), Some((_, upk::props::Value::Bool(false)))) {
                continue;
            }
            let density = match upk::registry::find(&ps, "Density") {
                Some((_, upk::props::Value::Float(f))) => *f,
                _ => 0.00005,
            };
            let col = match upk::registry::find(&ps, "LightColor") {
                Some((_, upk::props::Value::Color(c))) => Color::srgb_u8(c[2], c[1], c[0]),
                _ => Color::WHITE,
            };
            // per Unreal unit -> per metre
            out.fog = Some((density / crate::convert::S, col));
            best_height = height;
            info!("height fog: density {density} per uu ({:.4}/m)", density / crate::convert::S);
        }
    }
    // PostProcessVolume Settings (fields with an off-by-default bOverride keep the level's)
    for (name, st) in volume_settings {
        let v = crate::post::PostSettings::from_props(&st, out.post.as_ref());
        info!("post volume {name}: {v:?}");
        out.volume_post.insert(name, v);
    }
    out
}

/// Put the look on the game camera once it exists.
pub fn apply(mut look: ResMut<LevelLook>, mut cams: Query<(Entity, Option<&mut DistanceFog>), With<crate::goat::GoatCamera>>, mut commands: Commands) {
    if look.applied {
        return;
    }
    let Ok((cam, fog)) = cams.single_mut() else { return };
    look.applied = true;
    if std::env::var_os("GOAT_NO_LOOK").is_some() {
        return;
    }
    if let Some(post) = look.post.clone().filter(|_| std::env::var_os("GOAT_NO_POST").is_none()) {
        // the post pass reads the depth buffer
        commands.entity(cam).insert((
            post.uniform(0.0),
            Camera3d { depth_texture_usages: (bevy::render::render_resource::TextureUsages::RENDER_ATTACHMENT | bevy::render::render_resource::TextureUsages::TEXTURE_BINDING).into(), ..default() },
        ));
    }
    if let Some((contrast, sat)) = look.grading {
        let section = ColorGradingSection { contrast, ..default() };
        commands.entity(cam).insert(ColorGrading {
            global: ColorGradingGlobal { post_saturation: sat, ..default() },
            shadows: section,
            midtones: section,
            highlights: section,
        });
    }
    if let (Some((density, color)), Some(mut f)) = (look.fog, fog) {
        // the sky-tinted haze: the fog light colour a little darker so the distance doesn't glow
        let c = color.to_srgba();
        f.color = Color::srgba(c.red * 0.8, c.green * 0.85, c.blue * 0.92, 1.0);
        f.falloff = FogFalloff::Exponential { density };
    }
}

/// PostProcessVolumes: the highest-priority enabled one around the camera supplies the
/// settings (else the level's); changes blend over the new settings' DOF_InterpolationDuration.
pub fn post_volumes(
    time: Res<Time>,
    look: Res<LevelLook>,
    vols: Res<crate::physics::volumes::Volumes>,
    mut cams: Query<(&GlobalTransform, &mut crate::post::UberPost), With<crate::goat::GoatCamera>>,
    mut cur: Local<Option<crate::post::PostSettings>>,
    mut inside: Local<Option<String>>,
) {
    let _prof_scope = crate::prof::scope("look::post_volumes");
    let Some(base) = look.post.as_ref() else { return };
    let Ok((gt, mut u)) = cams.single_mut() else { return };
    if inside.is_none() && cur.is_none() && std::env::var_os("GOAT_TRACE").is_some() {
        for v in vols.others.iter().filter(|v| matches!(v.kind, crate::physics::volumes::Kind::PostProcess { .. })) {
            let (lo, hi) = v.hulls.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), (pose, sh)| {
                let a = sh.compute_aabb(pose);
                (lo.min(crate::convert::from_rapier(a.mins)), hi.max(crate::convert::from_rapier(a.maxs)))
            });
            println!("    post volume {} bounds {lo:.0?} .. {hi:.0?}", v.name);
        }
    }
    let p = gt.translation();
    let best = vols
        .others
        .iter()
        .filter_map(|v| match v.kind {
            crate::physics::volumes::Kind::PostProcess { priority } if v.contains(p, 0.0) => Some((priority, v.name.as_str())),
            _ => None,
        })
        .max_by_key(|(pr, _)| *pr)
        .map(|(_, n)| n.to_string());
    if best != *inside {
        info!("post-process volume: {}", best.as_deref().unwrap_or("none (level settings)"));
        *inside = best.clone();
    }
    let target = best.as_ref().and_then(|n| look.volume_post.get(n)).unwrap_or(base);
    let now = cur.get_or_insert_with(|| target.clone());
    if now != target {
        let k = if target.blend <= 0.0 { 1.0 } else { (time.delta_secs() / target.blend).min(1.0) };
        *now = now.lerp(target, k);
    }
    let t = u.time;
    *u = now.uniform(t);
}

pub struct LookPlugin;

impl Plugin for LookPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LevelLook>().add_systems(Update, (apply, post_volumes));
    }
}
