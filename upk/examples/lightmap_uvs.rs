//! Lightmap UV ranges of the meshes that lightmapped instances use (out-of-range UVs sample
//! other components' texels).
use std::collections::HashMap;
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map");
    let p = Package::open(&path)?;
    let lvl = upk::level::load(&p)?;
    let mut uses: HashMap<i32, (usize, bool)> = HashMap::new();
    for i in lvl.instances.iter().filter(|i| !i.hidden && i.lightmap.is_some()) {
        let e = uses.entry(i.mesh).or_default();
        e.0 += 1;
        e.1 |= i.matrix.is_some();
    }
    let mut bad = 0;
    for (&m, &(n, foliage)) in &uses {
        if m <= 0 { continue; }
        let m = m as usize - 1;
        let Ok(sm) = upk::mesh::StaticMesh::parse(&p, m) else { continue };
        let (_, ps) = upk::props::export_props(&p, m)?;
        let idx = match upk::props::find(&ps, "LightMapCoordinateIndex") { Some(upk::props::Value::Int(i)) => *i, _ => -1 };
        let uv = if sm.uvs2.is_empty() { &sm.uvs } else { &sm.uvs2 };
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for t in uv { for k in 0..2 { lo[k] = lo[k].min(t[k]); hi[k] = hi[k].max(t[k]); } }
        let out = lo[0] < -0.01 || lo[1] < -0.01 || hi[0] > 1.01 || hi[1] > 1.01;
        if out || foliage { 
            if out { bad += 1; }
            println!("{:40} x{n:5} foliage={foliage} lmidx={idx} sets2={} range {lo:.2?}..{hi:.2?}{}", sm.name, !sm.uvs2.is_empty(), if out { "  OUT" } else { "" });
        }
    }
    println!("{} meshes, {bad} with out-of-range lightmap UVs", uses.len());
    Ok(())
}
