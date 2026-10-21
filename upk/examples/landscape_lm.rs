//! Landscape component lightmaps: rect in texels vs component size.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map");
    let p = Package::open(&path)?;
    let mut shown = 0;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "LandscapeComponent" { continue; }
        let (end, ps) = upk::props::export_props(&p, i)?;
        let d = p.export_data(i);
        // FLightMap2D directly: [type][guids][...]
        let mut buf = vec![1u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        buf.extend_from_slice(&d[end..]);
        let Some(lm) = upk::level::component_lightmap(&buf) else { println!("{}: no lightmap", p.obj_name(i as i32 + 1)); continue };
        let t = upk::texture::Texture::parse(&p, lm.textures[0] as usize - 1)?;
        let base = |k: &str| match upk::props::find(&ps, k) { Some(upk::props::Value::Int(v)) => *v, _ => 0 };
        println!("{:?} {:?} {} base ({},{}) atlas {}x{} rect {:.2}x{:.2} texels at ({:.2},{:.2}) scales {:?}", lm.textures, lm.textures.map(|t| if t > 0 { p.obj_name(t) } else { String::new() }), p.obj_name(i as i32 + 1), base("SectionBaseX"), base("SectionBaseY"), t.width, t.height,
            lm.coord_scale[0] * t.width as f32, lm.coord_scale[1] * t.height as f32, lm.coord_bias[0] * t.width as f32, lm.coord_bias[1] * t.height as f32, lm.scales);
        shown += 1;
        if shown >= 6 { break; }
    }
    Ok(())
}
