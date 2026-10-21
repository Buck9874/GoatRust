//! Probe: StaticMeshComponent native data (LODData / lightmaps).
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map");
    let class = std::env::args().nth(2).unwrap_or("StaticMeshComponent".into());
    let p = Package::open(&path)?;
    let mut shown = 0;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != class { continue; }
        let (end, _) = upk::props::export_props(&p, i)?;
        let d = p.export_data(i);
        let rest = &d[end..];
        if rest.len() < 8 { continue; }
        let ints: Vec<i32> = rest.chunks_exact(4).take(40).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect();
        let names: Vec<String> = ints.iter().map(|&v| if v != 0 && v.unsigned_abs() as usize <= p.exports.len().max(p.imports.len()) { let c = p.class_name(v); if c.contains("Texture") || c.contains("Shadow") || c.contains("Light") { format!("{v}={c}") } else { String::new() } } else { String::new() }).collect();
        println!("{} native {}: {:?}", p.obj_path(i as i32 + 1), rest.len(), ints);
        println!("   refs: {:?}", names.iter().enumerate().filter(|(_, s)| !s.is_empty()).collect::<Vec<_>>());
        shown += 1;
        if shown >= 4 { break; }
    }
    Ok(())
}
