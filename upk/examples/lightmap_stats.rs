//! Lightmap atlases referenced by a map's mesh instances: count, sizes, texels.
use std::collections::{HashMap, HashSet};
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map");
    let p = Package::open(&path)?;
    let lvl = upk::level::load(&p)?;
    let mut atlases: HashMap<(i32, i32), usize> = HashMap::new();
    let (mut with, mut without) = (0, 0);
    for i in lvl.instances.iter().filter(|i| !i.hidden) {
        match i.lightmap {
            Some(lm) => { with += 1; *atlases.entry((lm.textures[0], lm.textures[1])).or_default() += 1; }
            None => without += 1,
        }
    }
    let mut sizes: HashMap<(i32, i32), usize> = HashMap::new();
    let mut texels = 0u64;
    let mut t0s = HashSet::new();
    for &(t0, _) in atlases.keys() {
        t0s.insert(t0);
        let t = upk::texture::Texture::parse(&p, t0 as usize - 1)?;
        *sizes.entry((t.width, t.height)).or_default() += 1;
        texels += (t.width * t.height) as u64;
    }
    println!("instances: {with} lightmapped, {without} not; {} atlas pairs ({} distinct t0)", atlases.len(), t0s.len());
    println!("sizes: {sizes:?}; {:.1} Mtexels", texels as f64 / 1e6);
    for (c, xf, ps) in &lvl.lights {
        if c.contains("Directional") || c.contains("Sky") {
            println!("{c} rot {:?} {:?}", xf.rotation, ps.iter().map(|p| format!("{}={:?}", p.name, p.value)).collect::<Vec<_>>());
        }
    }
    let lt = lvl.landscapes.len();
    println!("landscapes {lt}, bsp {}", lvl.bsp.is_some());
    Ok(())
}
