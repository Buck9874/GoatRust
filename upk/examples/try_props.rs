//! Try parsing an export's properties at a range of start offsets.
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    let from: usize = a[3].parse()?;
    for i in 0..p.exports.len() {
        if p.obj_name(i as i32 + 1) != a[2] { continue; }
        let d = p.export_data(i);
        for s in from..from + 16 {
            let mut r = upk::reader::Reader::at(d, s);
            match upk::props::parse_props(&p, &mut r) {
                Ok(ps) => { println!("start {s}: {} props end {}", ps.len(), r.pos); for x in ps { println!("  {} = {:?}", x.name, x.value); } return Ok(()); }
                Err(e) => println!("start {s}: {e}"),
            }
        }
    }
    Ok(())
}
