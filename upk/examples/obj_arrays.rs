//! Print an export's object-array properties with the referenced object names.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_name(i as i32 + 1) != a[2] { continue; }
        let (_, ps) = upk::props::export_props(&p, i)?;
        for pr in &ps {
            let objs = upk::material::object_array(Some(&pr.value));
            if !objs.is_empty() { println!("{} = {:?}", pr.name, objs.iter().map(|&o| p.obj_path(o)).collect::<Vec<_>>()); }
        }
    }
    Ok(())
}
