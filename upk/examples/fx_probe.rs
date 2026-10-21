//! Emitters of a ParticleSystem (by path) with their materials/textures.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let d = std::path::Path::new(&a[1]);
    let mut pk = vec![Package::open(d.join("Maps/Level_01.goat"))?];
    for f in ["GoatGame.u", "Startup.upk", "Engine.u"] { pk.push(Package::open(d.join(f))?); }
    let reg = upk::registry::Registry::new(pk);
    let r = reg.find_path(&a[2]).expect("not found");
    let sys = upk::particles::load_system(&reg, r).expect("load");
    for e in &sys.emitters {
        let tex = e.texture.map(|t| format!("{} [{}]", reg.path(t), upk::texture::Texture::parse(reg.pkg(t), t.export).map(|x| format!("{} mips {} tfc {:?}", x.format, x.mips.len(), x.tfc)).unwrap_or_else(|e| e.to_string())));
        println!("{:30} mat {:?} tex {:?} blend {:?} unsupported {:?}", e.name, e.material.map(|m| reg.path(m)), tex, e.blend, e.unsupported);
    }
    Ok(())
}
