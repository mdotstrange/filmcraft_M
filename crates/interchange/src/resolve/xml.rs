//! The XML dialect of a `.drp`: each document is one database object tree. An object is
//! `<ClassName DbId="uuid">` with one child element per property; a collection property is
//! `<Prop><Element>…</Element></Prop>`; a reference is either the uuid as text or the referenced
//! object inlined; an empty element is a null. Class names may contain `::`
//! (`<ListMgt::LmPowerNodeList>`), which an XML parser reads as a broken namespace prefix, so the
//! text is sanitised ([`sanitise_tags`]) before `roxmltree` sees it.

use roxmltree::Node;

use super::blob;
use crate::xml::{child, text};

/// Replace `::` with `__` inside tags (never in text, attribute values, comments, CDATA or
/// processing instructions), so `<ListMgt::LmPowerNodeList>` becomes a well-formed name.
pub fn sanitise_tags(xml: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Text,
        Tag,
        Quoted(u8),
        Comment,
        Cdata,
        Pi,
    }
    let b = xml.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut state = State::Text;
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        match state {
            State::Text => {
                if c == b'<' {
                    state = if b.get(i..i + 4) == Some(b"<!--") {
                        State::Comment
                    } else if b.get(i..i + 9) == Some(b"<![CDATA[") {
                        State::Cdata
                    } else if b.get(i + 1) == Some(&b'?') {
                        State::Pi
                    } else {
                        State::Tag
                    };
                }
                out.push(c);
                i += 1;
            }
            State::Tag => {
                if c == b'"' || c == b'\'' {
                    state = State::Quoted(c);
                    out.push(c);
                    i += 1;
                } else if c == b'>' {
                    state = State::Text;
                    out.push(c);
                    i += 1;
                } else if c == b':' && b.get(i + 1) == Some(&b':') {
                    out.extend_from_slice(b"__");
                    i += 2;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            State::Quoted(q) => {
                if c == q {
                    state = State::Tag;
                }
                out.push(c);
                i += 1;
            }
            State::Comment => {
                if b.get(i..i + 3) == Some(b"-->") {
                    state = State::Text;
                    out.extend_from_slice(b"-->");
                    i += 3;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            State::Cdata => {
                if b.get(i..i + 3) == Some(b"]]>") {
                    state = State::Text;
                    out.extend_from_slice(b"]]>");
                    i += 3;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
            State::Pi => {
                if b.get(i..i + 2) == Some(b"?>") {
                    state = State::Text;
                    out.extend_from_slice(b"?>");
                    i += 2;
                } else {
                    out.push(c);
                    i += 1;
                }
            }
        }
    }
    // Only ASCII bytes were touched, so the result is still valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Parse a sanitised document.
pub fn parse(sanitised: &str) -> Result<roxmltree::Document<'_>, String> {
    crate::xml::parse(sanitised)
}

/// The class name of an object node (after sanitising, `ListMgt__LmPowerNodeList`).
pub fn class<'a>(n: Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

/// The object's database id.
pub fn db_id<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    n.attribute("DbId").map(str::trim).filter(|s| !s.is_empty())
}

/// Text of property `name` (trimmed), `None` when missing or empty (a null).
pub fn prop_text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    let t = text(child(n, name)?);
    (!t.is_empty()).then_some(t)
}

/// An integer property; a decimal value is rounded.
pub fn prop_i64(n: Node<'_, '_>, name: &str) -> Option<i64> {
    parse_i64(prop_text(n, name)?)
}

/// `"108000"` → 108000; `"3600.0"` → 3600; anything else `None`.
pub fn parse_i64(t: &str) -> Option<i64> {
    t.parse::<i64>().ok().or_else(|| t.parse::<f64>().ok().filter(|f| f.is_finite() && f.abs() < 9.0e15).map(|f| f.round() as i64))
}

pub fn prop_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    prop_text(n, name)?.parse::<f64>().ok().filter(|f| f.is_finite())
}

pub fn prop_bool(n: Node<'_, '_>, name: &str) -> Option<bool> {
    prop_text(n, name).map(|t| t.eq_ignore_ascii_case("true") || t == "1")
}

/// A hex-encoded binary property.
pub fn prop_hex(n: Node<'_, '_>, name: &str) -> Option<Vec<u8>> {
    blob::hex(prop_text(n, name)?)
}

/// The object inlined under property `name` (`<Prop><Class DbId=…>…</Class></Prop>`).
pub fn object_in<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    child(n, name)?.children().find(|c| c.is_element())
}

/// The `<Element>` children of collection property `name`.
pub fn elements_of<'a, 'i>(n: Node<'a, 'i>, name: &str) -> impl Iterator<Item = Node<'a, 'i>> {
    child(n, name).into_iter().flat_map(|p| p.children().filter(|c| c.is_element() && c.tag_name().name() == "Element"))
}

/// The object inlined in an `<Element>`, if it holds one.
pub fn element_object<'a, 'i>(e: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    e.children().find(|c| c.is_element())
}

/// The text of an `<Element>` (a uuid reference), when it holds no object.
pub fn element_text<'a>(e: Node<'a, '_>) -> Option<&'a str> {
    if element_object(e).is_some() {
        return None;
    }
    let t = text(e);
    (!t.is_empty()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitises_only_tag_names() {
        let s = sanitise_tags("<?xml version=\"1.0\"?>\n<!--Db::Ver-->\n<A::B DbId=\"x::y\"><C::D>t::ext</C::D><![CDATA[a::b]]><E/></A::B>");
        assert_eq!(s, "<?xml version=\"1.0\"?>\n<!--Db::Ver-->\n<A__B DbId=\"x::y\"><C__D>t::ext</C__D><![CDATA[a::b]]><E/></A__B>");
        let d = parse(&s).unwrap();
        let root = d.root_element();
        assert_eq!(class(root), "A__B");
        assert_eq!(db_id(root), Some("x::y"));
        assert_eq!(prop_text(root, "C__D"), Some("t::ext"));
        // unterminated constructs never panic
        for frag in ["<a::", "<!--", "<![CDATA[", "<?", "<a b='::", "::", "", "<>", "<a::b></a::b"] {
            let _ = sanitise_tags(frag);
        }
        // multi-byte text is untouched
        assert_eq!(sanitise_tags("<N>ü::ß</N>"), "<N>ü::ß</N>");
    }

    #[test]
    fn object_tree_helpers() {
        let s = sanitise_tags(
            "<Sm2MpFolder DbId=\"f1\"><Name>Master</Name><MpFolder/><Count>3.0</Count><Flag>true</Flag><Blob>0a0b</Blob>\
             <MediaVec><Element><Sm2MpVideoClip DbId=\"m1\"><Name>a.mov</Name></Sm2MpVideoClip></Element><Element>uuid-2</Element></MediaVec>\
             <Video><BtVideoInfo DbId=\"v1\"/></Video><Ref>uuid-3</Ref></Sm2MpFolder>",
        );
        let d = parse(&s).unwrap();
        let r = d.root_element();
        assert_eq!(prop_text(r, "Name"), Some("Master"));
        assert_eq!(prop_text(r, "MpFolder"), None);
        assert_eq!(prop_text(r, "Missing"), None);
        assert_eq!(prop_i64(r, "Count"), Some(3));
        assert_eq!(prop_f64(r, "Count"), Some(3.0));
        assert_eq!(prop_bool(r, "Flag"), Some(true));
        assert_eq!(prop_hex(r, "Blob"), Some(vec![10, 11]));
        let els: Vec<_> = elements_of(r, "MediaVec").collect();
        assert_eq!(els.len(), 2);
        assert_eq!(class(element_object(els[0]).unwrap()), "Sm2MpVideoClip");
        assert_eq!(element_text(els[0]), None);
        assert_eq!(element_text(els[1]), Some("uuid-2"));
        assert_eq!(db_id(object_in(r, "Video").unwrap()), Some("v1"));
        assert!(object_in(r, "Ref").is_none());
        assert_eq!(parse_i64("-68719476737"), Some(-68_719_476_737));
        assert_eq!(parse_i64("1e400"), None);
        assert_eq!(parse_i64("x"), None);
    }
}
