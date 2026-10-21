//! Decode a Texture2D by path from a package (or the map + GoatGame + Startup) to PNG.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let d = std::path::Path::new(&a[1]);
    let mut pk = vec![Package::open(d.join("Maps/Level_01.goat"))?];
    for f in ["GoatGame.u", "Startup.upk", "Engine.u"] { pk.push(Package::open(d.join(f))?); }
    let reg = upk::registry::Registry::new(pk);
    let r = reg.find_path(&a[2]).expect("texture not found");
    let t = upk::texture::Texture::parse(reg.pkg(r), r.export)?;
    let img = t.decode(reg.pkg(r), r.export, 4096)?;
    println!("{} {}x{} {} srgb {}", reg.path(r), img.width, img.height, t.format, t.srgb);
    image::RgbaImage::from_raw(img.width, img.height, img.rgba).unwrap().save(&a[3])?;
    Ok(())
}
