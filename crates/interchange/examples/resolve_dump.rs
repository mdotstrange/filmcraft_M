//! Dump what FilmCraft reads from a DaVinci Resolve `.drp` export: the archive entries, every
//! object tree with its decoded binary fields, and the parsed model. A maintainer tool for
//! checking a project the importer gets wrong.
//!
//! ```text
//! cargo run -p filmcraft-interchange --example resolve_dump -- "path/to/Project.drp" [entry-filter]
//! ```

use std::fmt::Write as _;

use filmcraft_interchange::resolve::{blob, model, xml, zipfile};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: resolve_dump <file.drp> [entry-filter]");
        std::process::exit(2);
    };
    let filter = args.next();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let archive = match zipfile::Archive::parse(&bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    println!("{} entries", archive.entries.len());
    for entry in &archive.entries {
        if filter.as_ref().is_some_and(|f| !entry.name.contains(f.as_str())) {
            continue;
        }
        println!("\n==== {} ({} bytes, method {})", entry.name, entry.size, entry.method);
        let data = match archive.read(&bytes, entry) {
            Ok(d) => d,
            Err(e) => {
                println!("  !! {e}");
                continue;
            }
        };
        let text = String::from_utf8_lossy(&data);
        let sanitised = xml::sanitise_tags(&text);
        let doc = match xml::parse(&sanitised) {
            Ok(d) => d,
            Err(e) => {
                println!("  !! xml: {e}");
                continue;
            }
        };
        let mut out = String::new();
        dump(doc.root_element(), 0, &mut out);
        print!("{out}");
        let root = doc.root_element();
        match xml::class(root) {
            "SM_Project" => println!("  => {:?}", model::parse_project(&doc)),
            "Sm2MpFolder" => println!("  => {:#?}", model::parse_folder(&doc)),
            "Sm2SequenceContainer" => println!("  => {:#?}", model::parse_container(&doc)),
            _ => {}
        }
    }
}

fn dump(n: roxmltree::Node<'_, '_>, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth.min(40));
    let name = n.tag_name().name();
    let id = xml::db_id(n).map(|i| format!(" DbId={i}")).unwrap_or_default();
    let text = n.text().map(str::trim).unwrap_or("");
    let has_children = n.children().any(|c| c.is_element());
    if has_children {
        let _ = writeln!(out, "{pad}<{name}{id}>");
        for c in n.children().filter(|c| c.is_element()) {
            dump(c, depth + 1, out);
        }
        return;
    }
    if text.is_empty() {
        let _ = writeln!(out, "{pad}<{name}{id}/>");
        return;
    }
    if text.len() >= 16 && text.len().is_multiple_of(2) && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        let _ = writeln!(out, "{pad}<{name}{id}> [{} bytes] {}", text.len() / 2, describe(text));
    } else {
        let _ = writeln!(out, "{pad}<{name}{id}> {}", truncate(text, 160));
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// A one-line description of a hex blob, trying each known encoding.
fn describe(hex_text: &str) -> String {
    let Some(b) = blob::hex(hex_text) else { return "not hex".into() };
    if b.len() == 16
        && let (Some(f), Some(i)) = (blob::f64_pair_le(&b), blob::i64_pair_be(&b))
    {
        return format!("f64le {f:?} / i64be {i:?}");
    }
    if let Some(t) = blob::timemap(&b) {
        return format!("timemap {t:?}");
    }
    if let Ok(msg) = blob::v2_payload(&b) {
        return format!("v2 proto {}", proto(&msg, 0));
    }
    if b.first() == Some(&0x81)
        && let Ok(msg) = blob::zstd(b.get(1..).unwrap_or_default())
    {
        return format!("zstd proto {}", proto(&msg, 0));
    }
    if let Ok(m) = blob::qt_map(&b) {
        return format!("qt map {}", qt(&m));
    }
    if let Ok(d) = blob::qcompress(&b) {
        return format!("qCompress [{} bytes] {}", d.len(), truncate(&String::from_utf8_lossy(&d).replace(['\n', '\r'], " "), 120));
    }
    format!("? {}", truncate(hex_text, 48))
}

fn qt(m: &blob::Map) -> String {
    let mut s = String::from("{");
    for (k, v) in m {
        let _ = write!(s, " {k}: ");
        match v {
            blob::Value::Bytes(b) => {
                if let Ok(msg) = blob::v2_payload(b) {
                    let _ = write!(s, "v2 proto {}", proto(&msg, 0));
                } else if let Ok(inner) = blob::qt_map(b) {
                    s.push_str(&qt(&inner));
                } else if let Some(p) = blob::f64_pair_le(b).filter(|_| b.len() == 16) {
                    let _ = write!(s, "f64le {p:?} / i64be {:?}", blob::i64_pair_be(b));
                } else if let Ok(d) = blob::qcompress(b) {
                    let _ = write!(s, "qCompress[{}]", d.len());
                } else {
                    let _ = write!(s, "bytes[{}]", b.len());
                }
            }
            blob::Value::Str(Some(t)) => {
                let _ = write!(s, "{:?}", truncate(t, 60));
            }
            other => {
                let _ = write!(s, "{other:?}");
            }
        }
        s.push(',');
    }
    s.push_str(" }");
    truncate(&s, 600)
}

fn proto(msg: &[u8], depth: usize) -> String {
    let Ok(fields) = blob::proto_fields(msg) else { return format!("bytes[{}]", msg.len()) };
    let mut s = String::from("{");
    for (f, w) in fields {
        match w {
            blob::Wire::Varint(v) => {
                let _ = write!(s, " {f}={v}");
            }
            blob::Wire::Fixed64(v) => {
                let _ = write!(s, " {f}=f64:{}", f64::from_bits(v));
            }
            blob::Wire::Fixed32(v) => {
                let _ = write!(s, " {f}=f32:{}", f32::from_bits(v));
            }
            blob::Wire::Bytes(b) => {
                if let Some(t) = std::str::from_utf8(b).ok().filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
                    let _ = write!(s, " {f}={:?}", truncate(t, 80));
                } else if depth < 6 && !b.is_empty() && blob::proto_fields(b).is_ok() {
                    let _ = write!(s, " {f}={}", proto(b, depth + 1));
                } else {
                    let _ = write!(s, " {f}=bytes[{}]", b.len());
                }
            }
        }
    }
    s.push_str(" }");
    truncate(&s, 800)
}
