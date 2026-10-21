//! Disassemble the AS3 classes of a SWF whose name contains a filter (traits + method bytecode,
//! multinames and strings resolved).
//! usage: abc_dump <movie.swf> <class filter> [method filter]
use ruffle_core::swf::avm2::read::Reader;
use ruffle_core::swf::avm2::types::*;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&args[0])?;
    let filter = args.get(1).cloned().unwrap_or_default();
    let mfilter = args.get(2).cloned().unwrap_or_default();
    let buf = ruffle_core::swf::decompress_swf(&data[..])?;
    let swf = ruffle_core::swf::parse_swf(&buf)?;
    for tag in &swf.tags {
        let abc_data = match tag {
            ruffle_core::swf::Tag::DoAbc(d) => *d,
            ruffle_core::swf::Tag::DoAbc2(d) => d.data,
            _ => continue,
        };
        let abc = Reader::new(abc_data).read()?;
        let d = Dis { abc: &abc };
        for (ci, inst) in abc.instances.iter().enumerate() {
            let name = d.mn(inst.name.0);
            if !name.contains(&filter) {
                continue;
            }
            println!("class {name} extends {}", d.mn(inst.super_name.0));
            let cls = &abc.classes[ci];
            for (label, m) in [("cinit", cls.init_method.0), ("iinit", inst.init_method.0)] {
                if mfilter.is_empty() || label.contains(&mfilter) {
                    d.method(label, m);
                }
            }
            for (st, traits) in [("", &inst.traits), ("static ", &cls.traits)] {
                for t in traits {
                    let tn = d.mn(t.name.0);
                    match &t.kind {
                        TraitKind::Slot { type_name, .. } | TraitKind::Const { type_name, .. } => println!("  {st}var {tn}: {}", d.mn(type_name.0)),
                        TraitKind::Method { method, .. } | TraitKind::Getter { method, .. } | TraitKind::Setter { method, .. } => {
                            let kind = match t.kind {
                                TraitKind::Getter { .. } => "get ",
                                TraitKind::Setter { .. } => "set ",
                                _ => "",
                            };
                            if mfilter.is_empty() || tn.contains(&mfilter) {
                                d.method(&format!("{st}{kind}{tn}"), method.0);
                            } else {
                                println!("  {st}function {kind}{tn}");
                            }
                        }
                        _ => println!("  {st}{tn} {:?}", t.kind),
                    }
                }
            }
        }
        // script initialisers (top-level code), only without a filter match on classes
        if filter == "@scripts" {
            for (i, s) in abc.scripts.iter().enumerate() {
                d.method(&format!("script{i}"), s.init_method.0);
            }
        }
    }
    Ok(())
}

struct Dis<'a> {
    abc: &'a AbcFile,
}

impl Dis<'_> {
    fn s(&self, i: u32) -> String {
        if i == 0 { String::new() } else { String::from_utf8_lossy(&self.abc.constant_pool.strings[i as usize - 1]).to_string() }
    }
    fn ns(&self, i: u32) -> String {
        if i == 0 {
            return "*".into();
        }
        match &self.abc.constant_pool.namespaces[i as usize - 1] {
            Namespace::Package(n) | Namespace::PackageInternal(n) => self.s(n.0),
            Namespace::Private(_) => "private".into(),
            Namespace::Protected(_) | Namespace::StaticProtected(_) => "protected".into(),
            Namespace::Namespace(n) | Namespace::Explicit(n) => self.s(n.0),
        }
    }
    fn mn(&self, i: u32) -> String {
        if i == 0 {
            return "*".into();
        }
        match &self.abc.constant_pool.multinames[i as usize - 1] {
            Multiname::QName { namespace, name } | Multiname::QNameA { namespace, name } => {
                let ns = self.ns(namespace.0);
                if ns.is_empty() || ns == "private" || ns == "protected" { self.s(name.0) } else { format!("{ns}.{}", self.s(name.0)) }
            }
            Multiname::RTQName { name } | Multiname::RTQNameA { name } => self.s(name.0),
            Multiname::Multiname { name, .. } | Multiname::MultinameA { name, .. } => self.s(name.0),
            Multiname::TypeName { base_type, parameters } => {
                format!("{}.<{}>", self.mn(base_type.0), parameters.iter().map(|p| self.mn(p.0)).collect::<Vec<_>>().join(","))
            }
            other => format!("{other:?}"),
        }
    }
    fn method(&self, label: &str, m: u32) {
        let md = &self.abc.methods[m as usize];
        let params: Vec<String> = md.params.iter().map(|p| format!("{}: {}", p.name.map(|n| self.s(n.0)).unwrap_or_default(), self.mn(p.kind.0))).collect();
        println!("  function {label}({}): {}", params.join(", "), self.mn(md.return_type.0));
        let Some(body) = md.body else { return };
        let body = &self.abc.method_bodies[body.0 as usize];
        let mut r = Reader::new(&body.code);
        let re = regex_lite();
        loop {
            let pos = body.code.len() - r_remaining(&r, &body.code);
            let Ok(op) = r.read_op() else { break };
            let text = format!("{op:?}");
            println!("    {pos:5} {}", re(self, &text));
            if r_remaining(&r, &body.code) == 0 {
                break;
            }
        }
    }
}

fn r_remaining(r: &Reader, code: &[u8]) -> usize {
    use ruffle_core::swf::extensions::ReadSwfExt;
    code.len() - r.pos(code)
}

/// Replace `Index(N, PhantomData<...::Multiname>)` / `<...String>` with the resolved names.
fn regex_lite() -> impl Fn(&Dis, &str) -> String {
    |d: &Dis, t: &str| {
        let mut out = String::new();
        let mut rest = t;
        while let Some(i) = rest.find("Index(") {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 6..];
            let Some(comma) = tail.find(',') else { break };
            let n: u32 = tail[..comma].trim().parse().unwrap_or(0);
            let Some(close) = tail.find(">)") else { break };
            let ty = &tail[comma..close];
            if ty.ends_with("Multiname") {
                out.push_str(&format!("<{}>", d.mn(n)));
            } else if ty.ends_with("String") {
                out.push_str(&format!("{:?}", d.s(n)));
            } else {
                out.push_str(&format!("#{n}"));
            }
            rest = &tail[close + 2..];
        }
        out.push_str(rest);
        out
    }
}
