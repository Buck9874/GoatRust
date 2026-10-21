//! Dump compiled D3D11 pixel shaders whose reflection mentions a name (e.g. LightMapScale).
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("shader cache .upk");
    let needle = std::env::args().nth(2).unwrap_or("LightMapScale".into());
    let max: usize = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let p = Package::open(&path)?;
    std::fs::create_dir_all("out/shaders")?;
    let mut found = Vec::new();
    for i in 0..p.exports.len() {
        let d = p.export_data(i);
        let mut o = 0;
        while let Some(k) = d[o..].windows(4).position(|w| w == b"DXBC") {
            let s = o + k;
            o = s + 4;
            if s + 28 > d.len() { break; }
            let size = u32::from_le_bytes(d[s + 24..s + 28].try_into()?) as usize;
            if size < 32 || s + size > d.len() { continue; }
            let b = &d[s..s + size];
            if !b.windows(needle.len()).any(|w| w == needle.as_bytes()) { continue; }
            let code = b.windows(4).position(|w| w == b"SHEX" || w == b"SHDR");
            let Some(c) = code else { continue };
            let ver = u32::from_le_bytes(b[c + 8..c + 12].try_into()?);
            if ver >> 16 != 0 { continue; } // pixel shaders only
            found.push(b.to_vec());
        }
    }
    found.sort_by_key(|b| b.len());
    println!("{} pixel shaders mention {needle}: sizes {:?}", found.len(), found.iter().take(8).map(|b| b.len()).collect::<Vec<_>>());
    for (n, b) in found.iter().take(max).enumerate() {
        std::fs::write(format!("out/shaders/ps_{needle}_{n}.dxbc"), b)?;
    }
    Ok(())
}
