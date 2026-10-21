//! An export's object-array / object property resolved to paths: obj_array <package> <object path> <prop>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_path(i as i32 + 1) != a[2] {
            continue;
        }
        let (_, ps) = upk::props::export_props(&p, i)?;
        for x in ps.iter().filter(|x| x.name == a[3]) {
            match &x.value {
                upk::props::Value::Object(o) => println!("{} = {}", x.name, p.obj_path(*o)),
                v => {
                    for o in upk::material::object_array(Some(v)) {
                        println!("{}[] = {}", x.name, p.obj_path(o));
                    }
                }
            }
        }
    }
    Ok(())
}
