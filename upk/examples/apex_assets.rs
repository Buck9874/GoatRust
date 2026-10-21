//! Parse every ApexDestructibleAsset in a package and summarise it.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let p = Package::open(std::env::args().nth(1).expect("package"))?;
    let (mut ok, mut bad) = (0, 0);
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "ApexDestructibleAsset" { continue; }
        match upk::apex::parse(&p, i) {
            Ok(d) => {
                ok += 1;
                let leaves = d.leaf_chunks();
                let tris: usize = d.submeshes.iter().map(|s| s.indices.len() / 3).sum();
                let s0 = &d.submeshes[0];
                if std::env::var_os("APEX_PARAMS").is_some() {
                    let f: Vec<String> = d.params_raw.chunks_exact(4).take(24).map(|b| { let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]); let u = u32::from_le_bytes([b[0], b[1], b[2], b[3]]); if v.abs() > 1e-6 && v.abs() < 1e9 { format!("{v}") } else { format!("#{u:x}") } }).collect();
                    println!("{} params {}", d.name, f.join(" "));
                }
                println!("{:45} depth {} chunks {:3} leaves {:3} submeshes {} verts {} uvs {} tris {} parts {} thr {} r {} bounds {:.0?}",
                    p.obj_path(i as i32 + 1), d.depth_count, d.chunks.len(), leaves.len(), d.submeshes.len(), s0.positions.len(), s0.uvs.len(), tris, s0.index_partition.len(), d.damage_threshold, d.damage_to_radius, d.bounds);
            }
            Err(e) => { bad += 1; println!("{}: ERROR {e:#}", p.obj_path(i as i32 + 1)); }
        }
    }
    println!("{ok} parsed, {bad} failed");
    Ok(())
}
