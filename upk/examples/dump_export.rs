//! Write an export's native data (after its properties) to a file.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_path(i as i32 + 1) != a[2] { continue; }
        let (end, _) = upk::props::export_props(&p, i)?;
        std::fs::write(&a[3], &p.export_data(i)[end..])?;
        println!("wrote {} bytes", p.export_data(i).len() - end);
    }
    Ok(())
}
