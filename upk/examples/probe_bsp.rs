//! Probe: UModel (BSP) layout of a map's Model_0.
use upk::package::Package;
fn u32at(d: &[u8], o: usize) -> u32 { u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) }
fn f32at(d: &[u8], o: usize) -> f32 { f32::from_le_bytes(d[o..o + 4].try_into().unwrap()) }
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map path");
    let p = Package::open(&path)?;
    let i = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).ends_with("PersistentLevel.Model_0")).unwrap();
    let (end, _) = upk::props::export_props(&p, i)?;
    let d = p.export_data(i);
    println!("Model_0 {} bytes, props end {end}", d.len());
    let mut o = end;
    println!("bounds origin {:?} extent {:?} r {}", (f32at(d, o), f32at(d, o + 4), f32at(d, o + 8)), (f32at(d, o + 12), f32at(d, o + 16), f32at(d, o + 20)), f32at(d, o + 24));
    o += 28;
    for name in ["Vectors", "Points", "Nodes"] {
        let es = u32at(d, o) as usize;
        let n = u32at(d, o + 4) as usize;
        println!("{name}: elem {es} count {n} at {o}");
        o += 8 + es * n;
    }
    let owner = u32at(d, o);
    let ns = u32at(d, o + 4) as usize;
    println!("Surfs owner {owner} count {ns} at {o}");
    o += 8 + ns * 60;
    let es = u32at(d, o) as usize; let n = u32at(d, o + 4) as usize;
    println!("Verts elem {es} count {n}"); o += 8 + es * n;
    println!("NumSharedSides {} NumZones {}", u32at(d, o), u32at(d, o + 4));
    let nz = u32at(d, o + 4) as usize; o += 8;
    println!("next 64 after zones header: {:02x?}", &d[o..o + 64]);
    let _ = nz;
    // flags histogram + vert counts
    let nodes = 68012 + 8;
    let mut hist = std::collections::BTreeMap::new();
    for k in 0..ns { let f = u32at(d, 276660 + 8 + k * 60 + 4); *hist.entry(f).or_insert(0) += 1; }
    println!("polyflags: {hist:x?}");
    let mut nv = std::collections::BTreeMap::new();
    for k in 0..3260 { *nv.entry(d[nodes + k * 64 + 50]).or_insert(0) += 1; }
    println!("node NumVertices (byte 50): {nv:?}");
    for k in 0..6 {
        let b = &d[nodes + k * 64..nodes + k * 64 + 64];
        let ints: Vec<i32> = b.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
        let fl: Vec<f32> = b[..16].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        println!("node {k}: plane {fl:?} ints {:?}", &ints[4..]);
    }
    let verts = 276660 + 8 + 1196 * 60 + 8;
    for k in 0..4 {
        let b = &d[verts + k * 24..verts + k * 24 + 24];
        let ints: Vec<i32> = b.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
        println!("vert {k}: {ints:?}");
    }
    Ok(())
}
