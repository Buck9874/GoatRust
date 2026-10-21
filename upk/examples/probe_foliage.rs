//! Probe: native data after the properties of InstancedStaticMeshComponents (foliage).
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("map path");
    let p = Package::open(&path)?;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "InstancedStaticMeshComponent" {
            continue;
        }
        let (end, _) = upk::props::export_props(&p, i)?;
        let d = p.export_data(i);
        let rest = &d[end..];
        // find an int 80 followed by a count that fits
        let mut hit = None;
        for o in 0..rest.len().saturating_sub(8) {
            let e = u32::from_le_bytes(rest[o..o + 4].try_into()?);
            let n = u32::from_le_bytes(rest[o + 4..o + 8].try_into()?) as usize;
            if e == 80 && n > 0 && o + 8 + n * 80 <= rest.len() {
                hit = Some((o, n));
                break;
            }
        }
        println!("{} native {} bytes, bulk80 at {:?}, first 48: {:02x?}", p.obj_path(i as i32 + 1), rest.len(), hit, &rest[..rest.len().min(48)]);
        if let Some((o, n)) = hit {
            let f = |k: usize| f32::from_le_bytes(rest[o + 8 + k * 4..o + 12 + k * 4].try_into().unwrap());
            println!("   n={n} m0 = [{:.2} {:.2} {:.2} {:.2} | {:.2} {:.2} {:.2} {:.2} | {:.2} {:.2} {:.2} {:.2} | {:.1} {:.1} {:.1} {:.2}] tail after = {}", f(0), f(1), f(2), f(3), f(4), f(5), f(6), f(7), f(8), f(9), f(10), f(11), f(12), f(13), f(14), f(15), rest.len() - (o + 8 + n * 80));
        }
    }
    Ok(())
}
