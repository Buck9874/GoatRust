//! Hex dump the start of an export's data (by object name) with name-index hints.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let n: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
    for i in 0..p.exports.len() {
        if p.obj_name(i as i32 + 1) != a[2] { continue; }
        let d = p.export_data(i);
        println!("{} flags {:x} size {}", p.obj_path(i as i32 + 1), p.exports[i].object_flags, d.len());
        for (k, c) in d[..n.min(d.len())].chunks(4).enumerate() {
            if c.len() < 4 { break; }
            let v = i32::from_le_bytes(c.try_into().unwrap());
            let name = if v >= 0 && (v as usize) < p.names.len() { p.names[v as usize].clone() } else { String::new() };
            println!("{:4}: {:08x} {:>12} {:>14} {}", k * 4, v as u32, v, f32::from_bits(v as u32), name);
        }
        break;
    }
    Ok(())
}
