//! `file.importResolveProject` and `file.import` of a DaVinci Resolve `.drp` export: a new project
//! named after the Resolve project, or a bin in the open one; hostile parameters leave the project
//! as it was.

use serde_json::{Value, json};

use filmcraft_interchange::resolve::zipfile::write_stored;
use filmcraft_project::{BinEntry, ItemId};

use super::*;

fn demo() -> Session {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A Qt-serialised map of QString values.
fn qt_strings(entries: &[(&str, &str)]) -> String {
    fn qs(s: &str) -> Vec<u8> {
        let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut v = (units.len() as u32).to_be_bytes().to_vec();
        v.extend(units);
        v
    }
    let mut v = 1u32.to_be_bytes().to_vec();
    v.extend((entries.len() as u32).to_be_bytes());
    for (k, val) in entries {
        v.extend(qs(k));
        v.extend(10u32.to_be_bytes());
        v.push(0);
        v.extend(qs(val));
    }
    hexs(&v)
}

fn f64_pair(a: f64, b: f64) -> String {
    let mut v = a.to_le_bytes().to_vec();
    v.extend(b.to_le_bytes());
    hexs(&v)
}

/// A `Clip` record (directory + file name) as a raw versioned payload.
fn clip_record(dir: &str, file: &str) -> String {
    let mut msg = vec![0x0a, dir.len() as u8];
    msg.extend_from_slice(dir.as_bytes());
    msg.push(0x12);
    msg.push(file.len() as u8);
    msg.extend_from_slice(file.as_bytes());
    let mut v = 2u32.to_be_bytes().to_vec();
    v.extend((msg.len() as u32 + 1).to_be_bytes());
    v.push(0x80);
    v.extend(&msg);
    hexs(&v)
}

/// The smallest export worth importing: one folder, one clip at `dir/file`, one timeline "TL"
/// at 30 fps holding that clip.
fn drp(project: &str, dir: &str, file: &str) -> Vec<u8> {
    let head = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!--DbAppVer=\"21.1.1.0010\" DbPrjVer=\"17\"-->\n";
    let p = format!(
        "{head}<SM_Project DbId=\"p\"><ProjectName>{project}</ProjectName><TimelineHandleVec><Element>tl</Element></TimelineHandleVec>\
         <CurrentTimelineIndex>0</CurrentTimelineIndex><PowerNodeList><ListMgt::LmPowerNodeList DbId=\"pn\"/></PowerNodeList></SM_Project>"
    );
    let folder = format!(
        "{head}<Sm2MpFolder DbId=\"f\"><Name>Master</Name><MpFolder/><MediaVec>\
         <Element><Sm2MpVideoClip DbId=\"m1\"><Name>{file}</Name><Video><BtVideoInfo DbId=\"v1\"><Clip>{}</Clip></BtVideoInfo></Video></Sm2MpVideoClip></Element>\
         <Element><Sm2MpTimelineClip DbId=\"m2\"><Name>TL</Name><TimelineSharedHandle><Sm2Timeline DbId=\"tl\"><Name>TL</Name><Sequence>\
         <Sm2Sequence DbId=\"s1\"><FieldsBlob>{}</FieldsBlob><MediaExtents>{}</MediaExtents><FrameRate>{}</FrameRate><Resolution>00000000000007800000000000000438</Resolution>\
         </Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element></MediaVec></Sm2MpFolder>",
        clip_record(dir, file),
        qt_strings(&[("SeqRef", "c1")]),
        f64_pair(3600.0, 10.0),
        f64_pair(30.0, 0.0)
    );
    let container = format!(
        "{head}<Sm2SequenceContainer DbId=\"c1\"><VideoTrackVec><Element><Sm2TiTrack DbId=\"t1\"><Type>0</Type><Flags>0</Flags><Sequence>s1</Sequence><Items>\
         <Element><Sm2TiVideoClip DbId=\"i1\"><Name>{file}</Name><Start>108000</Start><Duration>60</Duration><In/><MediaRef>m1</MediaRef><MediaStartTime>0</MediaStartTime></Sm2TiVideoClip></Element>\
         </Items></Sm2TiTrack></Element></VideoTrackVec><AudioTrackVec/><SubtitleTrackVec/></Sm2SequenceContainer>"
    );
    write_stored(&[
        ("project.xml", p.as_bytes()),
        ("MediaPool/Master/MpFolder.xml", folder.as_bytes()),
        ("SeqContainer/c1.xml", container.as_bytes()),
        ("Gallery.xml", b"<Gallery::GyGallery DbId=\"g\"/>"),
    ])
}

struct Fixture {
    dir: std::path::PathBuf,
    drp: String,
    media: String,
}

fn fixture(tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("fc-resolve-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir_s = dir.to_string_lossy().replace('\\', "/");
    let media = format!("{dir_s}/missing clip.mov");
    let path = dir.join("Engine Test.drp");
    std::fs::write(&path, drp("Engine Test Project", &dir_s, "missing clip.mov")).unwrap();
    Fixture { dir, drp: path.to_string_lossy().to_string(), media }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn sequence_named(s: &Session, name: &str) -> Option<ItemId> {
    s.project.items.values().find(|i| i.name == name && i.as_sequence().is_some()).map(|i| i.id)
}

#[test]
fn drp_is_detected_as_an_interchange_document() {
    let f = fixture("detect");
    let bytes = std::fs::read(&f.drp).unwrap();
    assert_eq!(crate::interchange::detect(&f.drp, &bytes), Some(filmcraft_interchange::Format::ResolveProject));
    assert_eq!(crate::interchange::detect("x.txt", &bytes), None, "the extension is a gate");
}

#[test]
fn file_import_puts_the_export_into_the_chosen_bin() {
    let f = fixture("import");
    let mut s = demo();
    let before = s.project.items.len();
    let bin = std::sync::Arc::make_mut(&mut s.project).add_bin("From Resolve", None);
    let r = s.execute("file.import", json!({"paths": [f.drp], "bin": bin.0})).unwrap();
    let seq = ItemId(r["sequences"][0].as_u64().expect("a sequence"));
    assert_eq!(s.state.active_sequence, Some(seq), "the current timeline opens");
    assert_eq!(s.project.item(seq).unwrap().name, "TL");
    assert_eq!(s.project.root.parent_of(seq), Some(bin), "the fragment lands in the import bin");
    assert!(s.project.items.len() > before);
    let docs = r["documents"].as_array().unwrap();
    assert_eq!(docs[0]["format"], "DaVinci Resolve Project");
    let offline: Vec<&str> = docs[0]["offlineMedia"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(offline, [f.media.as_str()], "the clip's file does not exist, so it is reported offline");
    assert!(r["errors"].as_array().unwrap().is_empty());
}

#[test]
fn import_resolve_project_opens_a_new_project_named_after_it() {
    let f = fixture("new");
    let mut s = demo();
    s.execute("file.newSequence", json!({"name": "Leftover"})).unwrap();
    let r = s.execute("file.importResolveProject", json!({"path": f.drp})).unwrap();
    assert_eq!(s.project.name, "Engine Test Project");
    assert!(s.path.is_none(), "a new, unsaved project");
    assert!(sequence_named(&s, "Leftover").is_none(), "the previous project is gone");
    let seq = sequence_named(&s, "TL").expect("the timeline");
    assert_eq!(s.state.active_sequence, Some(seq));
    assert_eq!(r["project"], "Engine Test Project");
    assert_eq!(r["mode"], "newProject");
    assert_eq!(r["sequences"][0].as_u64(), Some(seq.0));
    assert_eq!(r["offlineMedia"].as_array().unwrap().len(), 1);
    assert_eq!(s.history.undo.len(), 1, "the whole import is one undo step");
    assert_eq!(s.history.undo[0].0, "Import DaVinci Resolve Project");
    // the Media Pool root is the project root: the clip sits at the top level
    assert!(s.project.root.children.iter().any(|c| matches!(c, BinEntry::Item(i) if s.project.item(*i).is_some_and(|it| it.name == "missing clip.mov"))));
    s.project.sequence(seq).unwrap().check().unwrap();
}

#[test]
fn import_resolve_project_into_a_bin_keeps_the_open_project() {
    let f = fixture("bin");
    let mut s = demo();
    let demo_name = s.project.name.clone();
    let demo_seq = s.state.active_sequence.unwrap();
    let undo_before = s.history.undo.len();
    let r = s.execute("file.importResolveProject", json!({"path": f.drp, "mode": "intoBin"})).unwrap();
    assert_eq!(s.project.name, demo_name);
    assert!(s.project.item(demo_seq).is_some());
    let bin = s.project.root.children.iter().find_map(|c| match c {
        BinEntry::Bin(b) if b.name == "Engine Test Project" => Some(b),
        _ => None,
    });
    let bin = bin.expect("a bin named after the Resolve project");
    let seq = sequence_named(&s, "TL").unwrap();
    assert_eq!(s.project.root.parent_of(seq), Some(bin.id));
    assert_eq!(r["mode"], "intoBin");
    assert_eq!(s.history.undo.len(), undo_before + 1, "one undo step");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(sequence_named(&s, "TL").is_none(), "undo removes the whole import");
}

#[test]
fn hostile_parameters_leave_the_project_alone() {
    let f = fixture("hostile");
    let mut s = demo();
    let snapshot = s.project.clone();
    let revision = s.revision;
    let e = s.execute("file.importResolveProject", json!({})).unwrap_err();
    assert!(matches!(&e, EngineError::BadParams { cmd, .. } if cmd == "file.importResolveProject"), "{e:?}");
    let e = s.execute("file.importResolveProject", json!({"path": f.drp, "mode": "sideways"})).unwrap_err();
    assert!(matches!(&e, EngineError::BadParams { .. }), "{e:?}");
    let txt = f.dir.join("notes.txt");
    std::fs::write(&txt, b"<xmeml version=\"4\"/>").unwrap();
    let e = s.execute("file.importResolveProject", json!({"path": txt.to_string_lossy()})).unwrap_err();
    assert!(e.to_string().contains("not a DaVinci Resolve project"), "{e}");
    let e = s.execute("file.importResolveProject", json!({"path": f.dir.join("nope.drp").to_string_lossy()})).unwrap_err();
    assert!(e.to_string().contains("nope.drp"), "{e}");
    // a damaged export: the archive is cut short
    let mut bytes = std::fs::read(&f.drp).unwrap();
    bytes.truncate(bytes.len() / 2);
    let cut = f.dir.join("cut.drp");
    std::fs::write(&cut, &bytes).unwrap();
    assert!(s.execute("file.importResolveProject", json!({"path": cut.to_string_lossy()})).is_err());
    assert_eq!(*s.project, *snapshot, "nothing changed");
    assert_eq!(s.revision, revision);
}

#[test]
fn command_is_listed_under_file_import_from() {
    let spec = crate::commands::find("file.importResolveProject").expect("registered");
    assert_eq!(spec.menu, ["File", "Import From"]);
    assert_eq!(spec.label, "DaVinci Resolve Project…");
    assert!(spec.params.contains("\"path\""));
}
