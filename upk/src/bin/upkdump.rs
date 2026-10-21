//! upkdump <package> [summary|names|imports|exports|props <filter>|classes]
use anyhow::Result;
use upk::{Package, props};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("usage: upkdump <package> [cmd] [filter]");
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("summary");
    let filter = args.get(2).map(|s| s.to_lowercase());
    let p = Package::open(resolve(path))?;
    let s = &p.summary;
    match cmd {
        "summary" => {
            println!("{}: ver {} lic {} flags {:08x} compression {} chunks {}", p.name, s.version, s.licensee, s.package_flags, s.compression_flags, s.chunks.len());
            println!("names {} imports {} exports {} engine {} cooker {}", p.names.len(), p.imports.len(), p.exports.len(), s.engine_version, s.cooker_version);
            let mut counts = std::collections::BTreeMap::<String, usize>::new();
            for i in 0..p.exports.len() {
                *counts.entry(p.class_name(i as i32 + 1)).or_default() += 1;
            }
            let mut v: Vec<_> = counts.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            for (c, n) in v.iter().take(60) {
                println!("  {n:6} {c}");
            }
        }
        "names" => for (i, n) in p.names.iter().enumerate() { println!("{i} {n}") },
        "imports" => for i in 0..p.imports.len() { println!("{} {} {}", -(i as i32) - 1, p.class_name(-(i as i32) - 1), p.obj_path(-(i as i32) - 1)) },
        "exports" => for i in 0..p.exports.len() {
            let e = &p.exports[i];
            let path = p.obj_path(i as i32 + 1);
            let class = p.class_name(i as i32 + 1);
            if filter.as_ref().is_none_or(|f| path.to_lowercase().contains(f) || class.to_lowercase().contains(f)) {
                println!("{} {} {} size {} off {}", i + 1, class, path, e.serial_size, e.serial_offset);
            }
        },
        "props" => for i in 0..p.exports.len() {
            let path = p.obj_path(i as i32 + 1);
            let class = p.class_name(i as i32 + 1);
            if filter.as_ref().is_none_or(|f| path.to_lowercase().contains(f) || class.to_lowercase().contains(f)) {
                match props::export_props(&p, i) {
                    Ok((end, ps)) => {
                        println!("== {} {} (props end {} / {})", class, path, end, p.exports[i].serial_size);
                        for q in ps {
                            let idx = if q.index != 0 { format!("[{}]", q.index) } else { String::new() };
                            println!("  {}{} = {}", q.name, idx, props::fmt_value(&p, &q.value));
                        }
                    }
                    Err(e) => println!("== {} {} : props error {e}", class, path),
                }
            }
        },
        "scripts" => {
            // UnrealScript source kept in TextBuffer exports -> <outdir>/<Class>.uc
            let dir = std::path::PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "out/scripts".into()));
            std::fs::create_dir_all(&dir)?;
            let mut n = 0;
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "TextBuffer" { continue; }
                let (end, _) = props::export_props(&p, i)?;
                let mut r = upk::reader::Reader::at(p.export_data(i), end + 8);
                let text = r.fstring()?;
                let cls = p.obj_name(p.exports[i].outer);
                std::fs::write(dir.join(format!("{cls}.uc")), text.replace("\r\n", "\n"))?;
                n += 1;
            }
            println!("wrote {n} scripts to {}", dir.display());
        }
        "tex" => {
            // tex <name filter> [outdir] : decode matching Texture2D exports to PNG
            let f = filter.clone().unwrap_or_default();
            let dir = std::path::PathBuf::from(args.get(3).cloned().unwrap_or_else(|| "out/tex".into()));
            std::fs::create_dir_all(&dir)?;
            for i in 0..p.exports.len() {
                let class = p.class_name(i as i32 + 1);
                if !class.contains("Texture2D") || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                let res = upk::texture::Texture::parse(&p, i).and_then(|t| {
                    let img = t.decode(&p, i, 4096)?;
                    let path = dir.join(format!("{}.png", p.obj_name(i as i32 + 1)));
                    image::save_buffer(&path, &img.rgba, img.width, img.height, image::ExtendedColorType::Rgba8)?;
                    Ok(format!("{} {}x{} mips {} tfc {:?} -> {}x{}", t.format, t.width, t.height, t.mips.len(), t.tfc, img.width, img.height))
                });
                match res {
                    Ok(s) => println!("{} {}", p.obj_path(i as i32 + 1), s),
                    Err(e) => println!("{} ERROR {e:#}", p.obj_path(i as i32 + 1)),
                }
            }
        }
        "mesh" => {
            // mesh <filter> [obj outdir] : parse StaticMesh exports, optionally write OBJ
            let f = filter.clone().unwrap_or_default();
            let dir = args.get(3).map(std::path::PathBuf::from);
            let (mut ok, mut bad) = (0, 0);
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "StaticMesh" || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                match upk::mesh::StaticMesh::parse(&p, i) {
                    Ok(m) => {
                        ok += 1;
                        println!("{} verts {} tris {} sections {} uvsets {}", p.obj_path(i as i32 + 1), m.positions.len(), m.indices.len() / 3, m.sections.len(), if m.uvs2.is_empty() { 1 } else { 2 });
                        if let Some(d) = &dir {
                            std::fs::create_dir_all(d)?;
                            let mut s = String::new();
                            for v in &m.positions { s += &format!("v {} {} {}\n", v[0], v[2], -v[1]); }
                            for t in &m.uvs { s += &format!("vt {} {}\n", t[0], 1.0 - t[1]); }
                            for n in &m.normals { s += &format!("vn {} {} {}\n", n[0], n[2], -n[1]); }
                            for t in m.indices.chunks(3) { s += &format!("f {0}/{0}/{0} {1}/{1}/{1} {2}/{2}/{2}\n", t[0] + 1, t[1] + 1, t[2] + 1); }
                            std::fs::write(d.join(format!("{}.obj", m.name)), s)?;
                        }
                    }
                    Err(e) => { bad += 1; println!("{} ERROR {e:#}", p.obj_path(i as i32 + 1)); }
                }
            }
            println!("ok {ok} failed {bad}");
        }
        "level" => {
            let t = std::time::Instant::now();
            let l = upk::level::load(&p)?;
            println!("instances {} (dynamic {}) landscapes {} starts {} lights {} in {:?}", l.instances.len(), l.instances.iter().filter(|i| i.dynamic).count(), l.landscapes.len(), l.player_starts.len(), l.lights.len(), t.elapsed());
            for ls in &l.landscapes {
                println!("landscape {:?} comps {} layers {:?}", ls.xf, ls.comps.len(), ls.layers);
                let single = upk::material::Single(&p);
                if let Some(m) = (ls.material > 0).then(|| upk::registry::ObjRef { pkg: 0, export: ls.material as usize - 1 }) {
                    for (n, t, s) in upk::material::landscape_layers(&single, m) { println!("  layer {n} tex {} tiling {s}", p.obj_path(t.export as i32 + 1)); }
                }
                if let Some(c) = ls.comps.first() {
                    let mut sums = [0u64; 4];
                    for w in &c.weights { for k in 0..4 { sums[k] += w[k] as u64; } }
                    println!("  comp0 weight sums {sums:?} over {} verts", c.weights.len());
                }
            }
            for s in &l.player_starts { println!("start {:?}", s); }
            let mut mats = std::collections::BTreeMap::new();
            for inst in &l.instances { for &m in &inst.materials { if m != 0 { mats.insert(m, ()); } } }
            for (m, _) in mats.iter().take(15) { let r = upk::material::resolve(&p, *m); println!("mat {} -> {:?} {:?}", p.obj_path(*m), r.diffuse.map(|d| p.obj_path(d.export as i32 + 1)), r.color); }
        }
        "font" => {
            let f = filter.clone().unwrap_or_default();
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "Font" || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                match upk::font::Font::parse(&p, i) {
                    Ok(ft) => {
                        let (w, h) = ft.measure("Hello Goat");
                        println!("{} glyphs {} pages {:?} remap {} (remapped {}) max height {} 'Hello Goat' = {w}x{h}", p.obj_path(i as i32 + 1), ft.glyphs.len(), ft.pages, ft.remap.len(), ft.remapped, ft.max_height);
                        if let Some(g) = ft.glyph('A') { println!("  'A' = {g:?}"); }
                    }
                    Err(e) => println!("{} ERROR {e:#}", p.obj_path(i as i32 + 1)),
                }
            }
        }
        "skel" => {
            let f = filter.clone().unwrap_or_default();
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "SkeletalMesh" || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                match upk::skel::SkeletalMesh::parse(&p, i) {
                    Ok(m) => {
                        println!("{} origin {:?} rot {:?} bones {} verts {} tris {} sections {} mats {:?}", p.obj_path(i as i32 + 1), m.origin, m.rot_origin, m.bones.len(), m.positions.len(), m.indices.len() / 3, m.sections.len(), m.materials.iter().map(|&o| p.obj_path(o)).collect::<Vec<_>>());
                        if args.get(3).is_some() { for (k, b) in m.bones.iter().enumerate() { println!("  {k} {} parent {} pos {:?} rot {:?}", b.name, b.parent, b.position, b.rotation); } }
                    }
                    Err(e) => println!("{} ERROR {e:#}", p.obj_path(i as i32 + 1)),
                }
            }
        }
        "anims" => {
            let f = filter.clone().unwrap_or_default();
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "AnimSet" || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                match upk::anim::AnimSet::parse(&p, i) {
                    Ok(a) => {
                        println!("{} tracks {} seqs {}", p.obj_path(i as i32 + 1), a.track_bones.len(), a.sequences.len());
                        for s in &a.sequences {
                            let keys: usize = s.tracks.iter().map(|t| t.rot.len()).max().unwrap_or(0);
                            println!("  {:24} frames {:4} len {:6.2}s rate {} loop {} maxkeys {}", s.name, s.num_frames, s.length, s.rate_scale, s.looping, keys);
                        }
                        if let Some(s) = a.sequences.iter().find(|s| s.name == "Idle" || s.name == "Run") {
                            for (k, t) in s.tracks.iter().enumerate().take(6) { println!("    {} {} pos0 {:?} rot0 {:?}", s.name, a.track_bones[k], t.pos[0], t.rot[0]); }
                        }
                    }
                    Err(e) => println!("{} ERROR {e:#}", p.obj_path(i as i32 + 1)),
                }
            }
        }
        "skelcheck" => {
            // Which quaternion convention puts each bone at the centroid of the vertices it drives?
            let i = p.find_export(filter.as_deref().unwrap_or("goat"), Some("SkeletalMesh")).expect("mesh");
            let m = upk::skel::SkeletalMesh::parse(&p, i)?;
            fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
                [a[3]*b[0]+a[0]*b[3]+a[1]*b[2]-a[2]*b[1], a[3]*b[1]-a[0]*b[2]+a[1]*b[3]+a[2]*b[0], a[3]*b[2]+a[0]*b[1]-a[1]*b[0]+a[2]*b[3], a[3]*b[3]-a[0]*b[0]-a[1]*b[1]-a[2]*b[2]]
            }
            fn qrot(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
                let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), [-q[0], -q[1], -q[2], q[3]]);
                [p[0], p[1], p[2]]
            }
            let mut cent = vec![([0f32; 3], 0f32); m.bones.len()];
            for (v, (j, w)) in m.positions.iter().zip(m.joints.iter().zip(m.weights.iter())) {
                if w[0] > 200 { let c = &mut cent[j[0] as usize]; for k in 0..3 { c.0[k] += v[k]; } c.1 += 1.0; }
            }
            for mode in 0..4 {
                let mut gq = vec![[0f32, 0.0, 0.0, 1.0]; m.bones.len()];
                let mut gp = vec![[0f32; 3]; m.bones.len()];
                for (k, b) in m.bones.iter().enumerate() {
                    let mut q = b.rotation;
                    let flip = match mode { 0 => false, 1 => k > 0, 2 => true, _ => k == 0 };
                    if flip { q[3] = -q[3]; }
                    if k == 0 { gq[k] = q; gp[k] = b.position; continue; }
                    let pa = b.parent as usize;
                    let r = qrot(gq[pa], b.position);
                    gp[k] = [gp[pa][0] + r[0], gp[pa][1] + r[1], gp[pa][2] + r[2]];
                    gq[k] = qmul(gq[pa], q);
                }
                let mut err = 0.0; let mut n = 0.0;
                for (k, c) in cent.iter().enumerate() {
                    if c.1 < 5.0 { continue; }
                    let cc = [c.0[0] / c.1, c.0[1] / c.1, c.0[2] / c.1];
                    let d = ((cc[0]-gp[k][0]).powi(2) + (cc[1]-gp[k][1]).powi(2) + (cc[2]-gp[k][2]).powi(2)).sqrt();
                    err += d; n += 1.0;
                }
                println!("mode {mode}: mean bone-to-centroid distance {:.2} over {n} bones; head at {:?}", err / n, gp[5]);
            }
        }
        "phys" => {
            let i = p.find_export(filter.as_deref().unwrap_or("goat_Physics"), Some("PhysicsAsset")).expect("asset");
            let a = upk::physasset::PhysicsAsset::parse(&p, i)?;
            for b in &a.bodies { println!("body {} nocol {} {:?}", b.bone, b.no_collision, b.shapes.iter().map(|s| (&s.kind, s.tm)).collect::<Vec<_>>()); }
            for c in &a.constraints { println!("joint {} -> {} swing {}/{} twist {} limited {}/{}", c.bone1, c.bone2, c.swing1, c.swing2, c.twist, c.swing_limited, c.twist_limited); }
        }
        "structs" => {
            // structs <export filter> <array prop> : print each struct element of an array property
            let f = filter.clone().unwrap_or_default();
            let prop = args.get(3).cloned().unwrap_or_default();
            let exact = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase().ends_with(&f));
            if let Some(i) = exact.or_else(|| (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase().contains(&f))) {
                let (_, ps) = props::export_props(&p, i)?;
                let mut v = props::find(&ps, &prop).cloned();
                if v.is_none() { if let Some(props::Value::Struct(_, inner)) = props::find(&ps, "AggGeom") { v = props::find(inner, &prop).cloned(); } }
                for (k, e) in upk::material::struct_array(&p, v.as_ref()).iter().enumerate() {
                    println!("[{k}]");
                    for q in e { println!("  {} = {}", q.name, props::fmt_value(&p, &q.value)); }
                }
            }
        }
        "matcheck" => {
            // materials used by level meshes that resolve to no texture and no colour
            let l = upk::level::load(&p)?;
            let mut seen = std::collections::BTreeMap::<String, usize>::new();
            for inst in &l.instances {
                if let Ok(m) = upk::mesh::StaticMesh::parse(&p, (inst.mesh.max(1) - 1) as usize) {
                    for (k, s) in m.sections.iter().enumerate() {
                        let o = inst.materials.get(k).copied().filter(|&o| o != 0).unwrap_or(s.material);
                        let r = upk::material::resolve(&p, o);
                        if r.diffuse.is_none() && r.color.is_none() {
                            *seen.entry(format!("{} [{}] (mesh {})", p.obj_path(o), p.class_name(o), m.name)).or_default() += 1;
                        }
                    }
                }
            }
            let mut v: Vec<_> = seen.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1));
            for (k, n) in v.iter().take(40) { println!("{n:5} {k}"); }
            println!("{} unresolved material/mesh combos", v.len());
        }
        "sounds" => {
            // sounds <filter> [outdir] : extract SoundNodeWave Ogg streams
            let f = filter.clone().unwrap_or_default();
            let dir = std::path::PathBuf::from(args.get(3).cloned().unwrap_or_else(|| "out/sounds".into()));
            std::fs::create_dir_all(&dir)?;
            let single = upk::material::Single(&p);
            let (mut ok, mut bad) = (0, 0);
            for i in 0..p.exports.len() {
                if p.class_name(i as i32 + 1) != "SoundNodeWave" || !p.obj_path(i as i32 + 1).to_lowercase().contains(&f) { continue; }
                match upk::sound::wave(&single, upk::registry::ObjRef { pkg: 0, export: i }) {
                    Some(w) => {
                        // decode fully to prove the stream is valid Vorbis
                        let mut rd = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(w.ogg.clone()))?;
                        let mut samples = 0usize;
                        while let Some(pck) = rd.read_dec_packet_itl()? { samples += pck.len(); }
                        let secs = samples as f32 / rd.ident_hdr.audio_channels as f32 / rd.ident_hdr.audio_sample_rate as f32;
                        if (secs - w.duration).abs() > 0.1 { println!("{}: decoded {secs:.2}s vs Duration {:.2}s", w.name, w.duration); }
                        ok += 1;
                        std::fs::write(dir.join(format!("{}.ogg", w.name)), &w.ogg)?;
                    }
                    None => { bad += 1; println!("no ogg: {}", p.obj_path(i as i32 + 1)); }
                }
            }
            println!("extracted {ok}, failed {bad}");
        }
        "animinfo" => {
            // animinfo <export> : AnimationNames of every NPCAnimationInfo struct prop
            let f = filter.clone().unwrap_or_default();
            if let Some(i) = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase() == f) {
                let (_, ps) = props::export_props(&p, i)?;
                for q in &ps {
                    if let props::Value::Struct(sn, inner) = &q.value {
                        if sn == "NPCAnimationInfo" {
                            let snd = upk::material::object_array(props::find(inner, "SoundToPlay")).iter().map(|&o| p.obj_path(o)).collect::<Vec<_>>();
                            println!("{} {:?} speed {} sounds {:?}", q.name, upk::registry::name_array(&p, props::find(inner, "AnimationNames")), props::fmt_value(&p, props::find(inner, "MovementSpeed").unwrap_or(&props::Value::Float(0.0))), snd);
                        }
                    }
                }
            }
        }
        "strings_of" => {
            let f = filter.clone().unwrap_or_default();
            let prop = args.get(3).cloned().unwrap_or_default();
            if let Some(i) = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase() == f) {
                let (_, ps) = props::export_props(&p, i)?;
                if let Some(props::Value::Array(n, d)) = props::find(&ps, &prop) {
                    let mut r = upk::reader::Reader::new(d);
                    let v: Vec<String> = (0..*n).filter_map(|_| r.fstring().ok()).collect();
                    println!("{v:?}");
                }
            }
        }
        "archchain" => {
            // archchain <export path suffix> : archetype chain across map + GoatGame.u + Startup.upk
            let f = filter.clone().unwrap_or_default();
            let i = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase().ends_with(&f)).expect("export");
            let mut pkgs = vec![Package::open(resolve(path))?];
            for extra in ["GoatGame.u", "Startup.upk"] { pkgs.push(Package::open(upk::cooked_dir().join(extra))?); }
            let reg = upk::registry::Registry::new(pkgs);
            let mut cur = Some(upk::registry::ObjRef { pkg: 0, export: i });
            while let Some(r) = cur {
                let arch = reg.packages[r.pkg].exports[r.export].archetype;
                println!("pkg {} {} [{}] archetype idx {} ({})", r.pkg, reg.path(r), reg.class_name(r), arch, reg.packages[r.pkg].obj_path(arch));
                cur = reg.resolve(r.pkg, arch);
            }
        }
        "curve" => {
            // curve <export path> <InterpCurve prop> : its points as (InVal, OutVal)
            let f = filter.clone().unwrap_or_default();
            let prop = args.get(3).cloned().unwrap_or_default();
            if let Some(i) = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase() == f) {
                let (_, ps) = props::export_props(&p, i)?;
                if let Some(props::Value::Struct(_, inner)) = props::find(&ps, &prop) {
                    for pt in upk::material::struct_array(&p, props::find(inner, "Points")) {
                        let get = |k: &str| props::find(&pt, k).map(|v| props::fmt_value(&p, v)).unwrap_or_default();
                        println!("  {} -> {}", get("InVal"), get("OutVal"));
                    }
                }
            }
        }
        "names_of" => {
            // names_of <export filter> <array prop> : decode an FName array property
            let f = filter.clone().unwrap_or_default();
            let prop = args.get(3).cloned().unwrap_or_default();
            if let Some(i) = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_lowercase() == f) {
                let (_, ps) = props::export_props(&p, i)?;
                println!("{:?}", upk::registry::name_array(&p, props::find(&ps, &prop)));
            }
        }
        "matinee" => {
            // matinee <filter>: parsed InterpData (groups, movement keys sampled, events)
            let n = p.exports.len();
            let reg = upk::registry::Registry::new(vec![p]);
            for i in 0..n {
                let r = upk::registry::ObjRef { pkg: 0, export: i };
                if reg.class_name(r) != "InterpData" || !filter.as_ref().is_none_or(|f| reg.path(r).to_lowercase().contains(f)) {
                    continue;
                }
                let Some(d) = upk::matinee::load_interp(&reg, r) else { continue };
                println!("== {} length {:.2}", d.path, d.length);
                for g in &d.groups {
                    print!("  group {}:", g.name);
                    if let Some(m) = &g.movement {
                        print!(" move ({}; pos {} keys, euler {} keys, axes {:?})", if m.relative { "relative" } else { "world" }, m.pos.points.len(), m.euler.points.len(), m.axes.iter().map(|a| (a.0, a.1.points.len())).collect::<Vec<_>>());
                        for k in 0..=4 {
                            let t = d.length * k as f32 / 4.0;
                            let (pos, rot) = m.eval(t);
                            print!("\n    t={t:.2} pos ({:.0}, {:.0}, {:.0}) rot ({:.0}, {:.0}, {:.0})", pos[0], pos[1], pos[2], rot[0], rot[1], rot[2]);
                        }
                    }
                    if !g.events.is_empty() {
                        print!(" events {:?}", g.events);
                    }
                    println!();
                }
            }
        }
        "particles" => {
            // particles <filter>: parsed ParticleSystems (Engine.u supplies the module defaults)
            let n = p.exports.len();
            let mut pk = vec![p];
            if let Ok(e) = Package::open(resolve("Engine.u")) {
                pk.push(e);
            }
            let reg = upk::registry::Registry::new(pk);
            for i in 0..n {
                let r = upk::registry::ObjRef { pkg: 0, export: i };
                if reg.class_name(r) != "ParticleSystem" || !filter.as_ref().is_none_or(|f| reg.path(r).to_lowercase().contains(f)) {
                    continue;
                }
                let Some(sys) = upk::particles::load_system(&reg, r) else { continue };
                println!("== {} ({} emitters, warmup {})", sys.path, sys.emitters.len(), sys.warmup);
                for e in &sys.emitters {
                    let tex = e.texture.map(|t| reg.path(t)).unwrap_or_default();
                    println!("  {} {:?} tex {tex} dur {} loops {} align {} sub {}x{}/{} max {} {:?}", e.name, e.blend, e.duration, e.loops, e.align, e.sub_h, e.sub_v, e.sub_interp, e.max_particles, e.unsupported);
                    println!("    rate {:.1} x{:.2} bursts {:?}", e.rate.eval1(0.0, 0.5), e.rate_scale.eval1(0.0, 0.5), e.bursts);
                    for m in &e.modules {
                        let s = |d: &upk::particles::Dist| {
                            let a = d.eval(0.0, [0.0; 3]);
                            let b = d.eval(1.0, [1.0; 3]);
                            format!("[{:.2},{:.2},{:.2}]..[{:.2},{:.2},{:.2}]", a[0], a[1], a[2], b[0], b[1], b[2])
                        };
                        use upk::particles::Module as M;
                        let line = match m {
                            M::Lifetime(d) | M::Size(d) | M::Rotation(d) | M::RotationRate(d) | M::Location(d) | M::SubUV(d) | M::Drag(d) => s(d),
                            M::SizeMultiplyLife { d, .. } | M::SizeScale { d, .. } | M::Velocity { d, .. } | M::VelocityOverLife { d, .. } | M::Acceleration { d, .. } | M::AccelerationOverLife { d, .. } => s(d),
                            M::Color { c, a } | M::ColorOverLife { c, a } | M::ColorScaleOverLife { c, a, .. } => format!("{} a {}", s(c), s(a)),
                            M::Sphere { radius, .. } | M::Cylinder { radius, .. } => format!("radius {}", s(radius)),
                            M::Orbit { offset, rate, .. } => format!("offset {} rate {}", s(offset), s(rate)),
                            M::MeshRotation(d) | M::MeshRotationRate(d) => s(d),
                            M::SpawnPerUnit { per, unit, .. } => format!("{} per {unit} uu", s(per)),
                            M::Collision { damping, completion, .. } => format!("damping {} completion {completion}", s(damping)),
                            M::Other(n) => n.clone(),
                        };
                        let kind = format!("{m:?}");
                        let kind = kind.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                        println!("    {kind}: {line}");
                    }
                }
            }
        }
        "disasm" => {
            // disasm <Class.Function | Class.State.Function | Class> : decoded bytecode
            let target = args.get(2).expect("disasm <Class.Function>");
            let natives = native_table(&p);
            let nat = |i: u16| natives.get(&i).cloned();
            let f = upk::script::Fmt { p: &p, native: &nat };
            let mut list = vec![];
            if let Some(i) = upk::script::find_struct(&p, target) {
                if p.class_name(i as i32 + 1) == "Class" || p.class_name(i as i32 + 1) == "State" {
                    let s = upk::script::read_struct(&p, i)?;
                    list.extend(upk::script::children(&p, s.children).into_iter().filter(|&c| p.class_name(c) == "Function").map(|c| c as usize - 1));
                    list.push(i);
                } else {
                    list.push(i);
                }
            } else {
                anyhow::bail!("no function/state/class {target} in {}", p.name);
            }
            for i in list {
                let s = upk::script::read_struct(&p, i)?;
                let fi = s.func.clone().unwrap_or_default();
                println!("== {} {} (line {}, flags {:#x}, native {}, {} bytes disk / {} mem)", s.kind, p.obj_path(i as i32 + 1), s.line, fi.flags, fi.native, s.script.len(), s.mem_size);
                if s.super_ != 0 {
                    println!("   super {}", p.obj_path(s.super_));
                }
                if !s.interfaces.is_empty() {
                    println!("   implements {}", s.interfaces.iter().map(|&i| p.obj_name(i)).collect::<Vec<_>>().join(", "));
                }
                for c in upk::script::children(&p, s.children) {
                    if let Ok(q) = upk::script::read_property(&p, c as usize - 1) {
                        let refs: Vec<String> = q.refs.iter().map(|&r| p.obj_name(r)).collect();
                        println!("   {} {}{} flags {:#x} {}", q.kind, q.name, if q.array_dim > 1 { format!("[{}]", q.array_dim) } else { String::new() }, q.flags, refs.join(" "));
                    }
                }
                match upk::script::decode_struct(&p, &s) {
                    Ok(stmts) => for st in stmts { println!("  {:04x}: {}", st.mem, f.e(&st.expr)); },
                    Err(e) => {
                        let (part, _) = upk::script::decode_partial(&p, &s.script);
                        for st in part { println!("  {:04x}: {}  [disk {:#x}]", st.mem, f.e(&st.expr), st.disk); }
                        println!("  !! {e}");
                    }
                }
            }
        }
        "scriptcheck" => {
            // decode every struct's bytecode; report failures and memory-size mismatches
            let (mut ok, mut bad) = (0, 0);
            for i in 0..p.exports.len() {
                if !upk::script::is_struct_kind(&p.class_name(i as i32 + 1)) { continue; }
                let r = upk::script::read_struct(&p, i).and_then(|s| upk::script::decode_struct(&p, &s));
                match r {
                    Ok(_) => ok += 1,
                    Err(e) => {
                        bad += 1;
                        if bad <= filter.as_ref().and_then(|f| f.parse().ok()).unwrap_or(30) { println!("{}: {e}", p.obj_path(i as i32 + 1)); }
                    }
                }
            }
            println!("{}: {ok} ok, {bad} failed", p.name);
        }
        "hex" => {
            // hex <export number> [bytes]
            let i: usize = args[2].parse::<usize>()? - 1;
            let n: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
            let e = &p.exports[i];
            println!("{} {} flags {:016x}", p.class_name(i as i32 + 1), p.obj_path(i as i32 + 1), e.object_flags);
            for (row, c) in p.export_data(i).chunks(16).take(n.div_ceil(16)).enumerate() {
                let h: Vec<String> = c.iter().map(|b| format!("{b:02x}")).collect();
                println!("{:06x}: {}", row * 16, h.join(" "));
            }
        }
        _ => eprintln!("unknown cmd {cmd}"),
    }
    Ok(())
}

/// Indexed native names from Core/Engine (+ the package itself).
fn native_table(p: &Package) -> std::collections::HashMap<u16, String> {
    let mut m = std::collections::HashMap::new();
    upk::script::native_names(p, &mut m);
    for dep in ["Core.u", "Engine.u", "GameFramework.u"] {
        if !p.name.eq_ignore_ascii_case(dep.trim_end_matches(".u")) {
            if let Ok(d) = Package::open(upk::cooked_dir().join(dep)) {
                upk::script::native_names(&d, &mut m);
            }
        }
    }
    m
}

fn resolve(p: &str) -> std::path::PathBuf {
    let pb = std::path::PathBuf::from(p);
    if pb.exists() { pb } else { upk::cooked_dir().join(p) }
}
