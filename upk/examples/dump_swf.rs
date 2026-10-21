//! Dump SwfMovie RawData (Scaleform movies) from a package to out/swf/.
use upk::package::Package;
use upk::props::{self, Value};
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("package");
    let p = Package::open(&path)?;
    std::fs::create_dir_all("out/swf")?;
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "SwfMovie" { continue; }
        let (_, ps) = props::export_props(&p, i)?;
        if let Some(Value::Array(n, d)) = props::find(&ps, "RawData") {
            let name = p.obj_path(i as i32 + 1);
            std::fs::write(format!("out/swf/{name}.gfx"), d)?;
            println!("{name}: {n} bytes, header {:?} version {}", String::from_utf8_lossy(&d[..3]), d[3]);
        }
    }
    Ok(())
}
