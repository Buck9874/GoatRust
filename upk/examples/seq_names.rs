//! AnimSequence names inside an AnimSet: seq_names <package> <animset path>
use upk::package::Package;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let p = Package::open(&a[1])?;
    for i in 0..p.exports.len() {
        if p.obj_path(i as i32 + 1) == a[2] && p.class_name(i as i32 + 1) == "AnimSet" {
            let s = upk::anim::AnimSet::parse(&p, i)?;
            println!("{}", s.sequences.iter().map(|q| q.name.clone()).collect::<Vec<_>>().join(", "));
        }
    }
    Ok(())
}
