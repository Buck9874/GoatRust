//! Print InterpCurveFloat properties of an export: curve <package> <object path> <prop>...
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_path(i as i32 + 1) != a[2] {
            continue;
        }
        let (_, ps) = upk::props::export_props(&p, i)?;
        for name in &a[3..] {
            for x in ps.iter().filter(|x| &x.name == name) {
                if let upk::props::Value::Struct(_, inner) = &x.value {
                    let pts = upk::material::struct_array(&p, upk::props::find(inner, "Points"));
                    let v: Vec<String> = pts
                        .iter()
                        .map(|pt| format!("{:?}->{:?}", upk::props::find(pt, "InVal"), upk::props::find(pt, "OutVal")))
                        .collect();
                    println!("{name}: {}", v.join(", "));
                }
            }
        }
    }
    Ok(())
}
