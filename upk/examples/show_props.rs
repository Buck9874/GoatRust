//! Print an export's tagged properties: show_props <package> <object name>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_name(i as i32 + 1) != a[2] {
            continue;
        }
        let (_, ps) = upk::props::export_props(&p, i)?;
        println!("{}", p.obj_path(i as i32 + 1));
        for x in ps {
            println!("  {} = {:?}", x.name, x.value);
        }
    }
    Ok(())
}
