//! DaVinci Resolve `.drp` import: a synthetic export built in-test (stored ZIP of the XML
//! documents Resolve writes), plus real exports when `FILMCRAFT_RESOLVE_DRP` names them.

mod support;

use std::collections::HashSet;

use filmcraft_interchange::resolve::zipfile::write_stored;
use filmcraft_interchange::{Format, ImportOptions, Imported, Report, detect, export, import_with, merge_into};
use filmcraft_media::Generator;
use filmcraft_project::{BinEntry, ItemId, ItemKind, MediaRef, Project, Sequence, TransitionAlign};
use filmcraft_time::{FrameRate, Tick};

// ---------------------------------------------------------------- blob builders

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn qs(s: &str) -> Vec<u8> {
    let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut v = (units.len() as u32).to_be_bytes().to_vec();
    v.extend(units);
    v
}
fn var(t: u32, payload: &[u8]) -> Vec<u8> {
    let mut v = t.to_be_bytes().to_vec();
    v.push(0);
    v.extend_from_slice(payload);
    v
}
fn int(i: i32) -> Vec<u8> {
    var(2, &i.to_be_bytes())
}
fn uint(u: u32) -> Vec<u8> {
    var(3, &u.to_be_bytes())
}
fn long(i: i64) -> Vec<u8> {
    var(4, &i.to_be_bytes())
}
fn string(s: &str) -> Vec<u8> {
    var(10, &qs(s))
}
fn bytes_var(payload: &[u8]) -> Vec<u8> {
    let mut v = (payload.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(payload);
    var(12, &v)
}
fn qmap(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut v = 1u32.to_be_bytes().to_vec();
    v.extend((entries.len() as u32).to_be_bytes());
    for (k, val) in entries {
        v.extend(qs(k));
        v.extend(val);
    }
    v
}
fn f64_pair(a: f64, b: f64) -> String {
    let mut v = a.to_le_bytes().to_vec();
    v.extend(b.to_le_bytes());
    hexs(&v)
}
fn i64_pair(a: i64, b: i64) -> Vec<u8> {
    let mut v = a.to_be_bytes().to_vec();
    v.extend(b.to_be_bytes());
    v
}
/// A raw (flag 0x80) versioned payload around a protobuf message.
fn v2(msg: &[u8]) -> String {
    let mut v = 2u32.to_be_bytes().to_vec();
    v.extend((msg.len() as u32 + 1).to_be_bytes());
    v.push(0x80);
    v.extend_from_slice(msg);
    hexs(&v)
}
fn pb_str(field: u8, s: &str) -> Vec<u8> {
    let mut v = vec![(field << 3) | 2, s.len() as u8];
    v.extend_from_slice(s.as_bytes());
    v
}
/// A `Clip` record: directory and file name.
fn clip_record(dir: &str, file: &str) -> String {
    let mut msg = pb_str(1, dir);
    msg.extend(pb_str(2, file));
    v2(&msg)
}
/// A subtitle generator's fields: field 1 → field 14 text.
fn subtitle_fields(text: &str) -> String {
    let inner = pb_str(14, text);
    let mut msg = vec![0x0a, inner.len() as u8];
    msg.extend(inner);
    v2(&msg)
}
fn time_blob(frames: i32, fps: f64) -> String {
    hexs(&qmap(&[
        ("NumFrames", int(frames)),
        ("StartFrame", int(0)),
        (
            "FrameRate",
            bytes_var(&{
                let mut v = fps.to_le_bytes().to_vec();
                v.extend(0f64.to_le_bytes());
                v
            }),
        ),
    ]))
}
fn geometry_blob(w: i64, h: i64) -> String {
    hexs(&qmap(&[("Resolution", bytes_var(&i64_pair(w, h)))]))
}
fn tracks_blob(sr: u32, ch: i32, samples: i64) -> String {
    let t = qmap(&[("SampleRate", uint(sr)), ("NumChannels", int(ch)), ("Duration", long(samples))]);
    hexs(&qmap(&[("0", bytes_var(&t))]))
}
fn seq_fields(seq_ref: &str) -> String {
    hexs(&qmap(&[("SeqRef", string(seq_ref))]))
}

// ---------------------------------------------------------------- the synthetic export

const HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!--DbAppVer=\"21.1.1.0010\" DbPrjVer=\"17\"-->\n";

fn video_clip(id: &str, name: &str, dir: Option<&str>, frames: i32, fps: f64, w: i64, h: i64, audio: bool) -> String {
    let clip = dir.map(|d| format!("<Clip>{}</Clip>", clip_record(d, name))).unwrap_or_default();
    let aud = if audio {
        format!(
            "<EmbeddedAudioVec><Element><BtAudioInfo DbId=\"{id}-a\">{clip}<TracksBA>{}</TracksBA></BtAudioInfo></Element></EmbeddedAudioVec>",
            tracks_blob(48_000, 2, 480_000)
        )
    } else {
        String::new()
    };
    format!(
        "<Element><Sm2MpVideoClip DbId=\"{id}\"><Name>{name}</Name><MarkIn/><MarkOut/>\
         <Video><BtVideoInfo DbId=\"{id}-v\">{clip}<Time>{}</Time><Geometry>{}</Geometry></BtVideoInfo></Video>{aud}</Sm2MpVideoClip></Element>",
        time_blob(frames, fps),
        geometry_blob(w, h)
    )
}

fn timeline_clip(id: &str, name: &str, tl: &str, seq: &str, seq_ref: &str, start_s: f64, dur_s: f64, fps: f64, size: (i64, i64)) -> String {
    format!(
        "<Element><Sm2MpTimelineClip DbId=\"{id}\"><Name>{name}</Name><TimelineSharedHandle><Sm2Timeline DbId=\"{tl}\"><Name>{name}</Name>\
         <Sequence><Sm2Sequence DbId=\"{seq}\"><FieldsBlob>{}</FieldsBlob><MediaExtents>{}</MediaExtents><FrameRate>{}</FrameRate>\
         <Resolution>{}</Resolution><NumOutputAudioChannels>2</NumOutputAudioChannels></Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element>",
        seq_fields(seq_ref),
        f64_pair(start_s, dur_s),
        f64_pair(fps, 0.0),
        hexs(&i64_pair(size.0, size.1))
    )
}

fn item(class: &str, id: &str, name: &str, start: i64, dur: i64, extra: &str) -> String {
    format!("<Element><{class} DbId=\"{id}\"><Name>{name}</Name><Start>{start}</Start><Duration>{dur}</Duration>{extra}</{class}></Element>")
}

fn track(id: &str, ty: u8, flags: u8, name: &str, seq: &str, items: &str) -> String {
    let name = if name.is_empty() { "<UserDefinedName/>".to_string() } else { format!("<UserDefinedName>{name}</UserDefinedName>") };
    format!(
        "<Element><Sm2TiTrack DbId=\"{id}\"><Type>{ty}</Type><SubType>0</SubType><Flags>{flags}</Flags>{name}<Sequence>{seq}</Sequence><Items>{items}</Items></Sm2TiTrack></Element>"
    )
}

/// The export: two timelines (the second nested in the first), a compound clip, a sub-folder,
/// generators, a title, transitions, a subtitle track, an offline clip and a clip whose media is
/// not in the Media Pool.
fn synthetic() -> Vec<u8> {
    let project = format!(
        "{HEAD}<SM_Project DbId=\"p1\"><FieldsBlob/><ProjectName>Synthetic Resolve Project</ProjectName>\
         <TimelineHandleVec><Element>tl-main</Element><Element>tl-nested</Element></TimelineHandleVec><CurrentTimelineIndex>0</CurrentTimelineIndex>\
         <PowerNodeList><ListMgt::LmPowerNodeList DbId=\"pn\"><FieldsBlob/></ListMgt::LmPowerNodeList></PowerNodeList></SM_Project>"
    );
    let music = format!(
        "<Element><Sm2MpAudioClip DbId=\"m-audio\"><Name>Music.wav</Name><EmbeddedAudioVec><Element><BtAudioInfo DbId=\"m-audio-a\"><Clip>{}</Clip><TracksBA>{}</TracksBA></BtAudioInfo></Element></EmbeddedAudioVec></Sm2MpAudioClip></Element>",
        clip_record("R:\\Assets\\", "Music.wav"),
        tracks_blob(48_000, 2, 2_400_000)
    );
    let compound = format!(
        "<Element><Sm2MpCompoundClip DbId=\"m-comp\"><Name>Comp</Name><Sequence><Sm2Sequence DbId=\"s-comp\"><FieldsBlob>{}</FieldsBlob><MediaExtents>{}</MediaExtents><FrameRate>{}</FrameRate><Resolution>{}</Resolution></Sm2Sequence></Sequence></Sm2MpCompoundClip></Element>",
        seq_fields("c-comp"),
        f64_pair(0.0, 5.0),
        f64_pair(24.0, 0.0),
        hexs(&i64_pair(1920, 1080))
    );
    let master = format!(
        "{HEAD}<Sm2MpFolder DbId=\"f-master\"><Name>Master</Name><MpFolder/><MediaVec>{}{music}{}{}{}{compound}\
         <Element><Sm2MpGenerator DbId=\"m-adj\"><Name>Adjustment Clip</Name><MpFolder>f-master</MpFolder></Sm2MpGenerator></Element></MediaVec></Sm2MpFolder>",
        video_clip("m-video", "Shot A.mov", Some("R:\\Assets"), 300, 30.0, 1920, 1080, true),
        video_clip("m-nopath", "Lost.mov", None, 90, 30.0, 1920, 1080, false),
        timeline_clip("m-main", "Main", "tl-main", "s-main", "c-main", 3600.0, 20.0, 30.0, (0, 0)),
        timeline_clip("m-nested", "Nested", "tl-nested", "s-nested", "c-nested", 3600.0, 10.0, 30.0, (1280, 720)),
    );
    let sub = format!(
        "{HEAD}<Sm2MpFolder DbId=\"f-sub\"><Name>Sub</Name><MpFolder>f-master</MpFolder><MediaVec>{}</MediaVec></Sm2MpFolder>",
        video_clip("m-still", "Still.png", Some("R:\\Assets\\Stills"), 150, 30.0, 640, 480, false)
    );
    let media_ref = |m: &str, path: &str, in_frames: Option<i64>| {
        format!(
            "<In>{}</In><MediaRef>{m}</MediaRef><MediaStartTime>0</MediaStartTime><MediaFilePath>{path}</MediaFilePath><MediaFrameRate>{}</MediaFrameRate>",
            in_frames.map(|i| i.to_string()).unwrap_or_default(),
            f64_pair(30.0, 0.0)
        )
    };
    let v1_items = [
        item(
            "Sm2TiVideoClip",
            "i-v1",
            "Shot A.mov",
            108000,
            100,
            &format!("{}<EffectFiltersBA>00</EffectFiltersBA>", media_ref("m-video", "R:\\Assets\\Shot A.mov", Some(10))),
        ),
        item("Sm2TiTransition", "i-x1", "Transition", 108100, 12, "<PrettyType>Cross Dissolve</PrettyType><AlignmentType>1</AlignmentType>"),
        item("Sm2TiVideoClip", "i-v2", "Nested", 108100, 50, "<In>20</In><MediaRef>m-nested</MediaRef><MediaStartTime>3600</MediaStartTime><MediaFilePath/>"),
        item("Sm2TiGenerator", "i-g1", "Solid Color", 108150, 30, "<PrettyType>Solid Color</PrettyType>"),
        item("Sm2TiVideoClip", "i-g2", "Title 1", 108180, 30, "<PrettyType>Fusion Title</PrettyType><MediaRef/><MediaFilePath/>"),
        item("Sm2TiVideoClip", "i-v3", "Lost.mov", 108210, 30, "<MediaRef>m-nopath</MediaRef>"),
        item("Sm2TiVideoClip", "i-v4", "Comp", 108240, 24, "<MediaRef>m-comp</MediaRef><MediaStartTime>0</MediaStartTime>"),
        item("Sm2TiVideoClip", "i-v5", "Extern.mov", 108264, 10, &media_ref("m-unknown", "R:\\Assets\\Extern.mov", None)),
        item("Sm2TiVideoClip", "i-v6", "Shot A.mov", 108000, 50, &media_ref("m-video", "R:\\Assets\\Shot A.mov", Some(0))),
    ]
    .concat();
    let v2_items = [
        item("Sm2TiVideoClip", "i-adj", "Adjustment Clip", 108200, 60, "<PrettyType>Adjustment Clip</PrettyType><In>108200</In><MediaRef/>"),
        item("Sm2TiVideoClip", "i-adj2", "Adjustment Clip", 108260, 30, "<PrettyType>Adjustment Clip</PrettyType><MediaRef>m-adj</MediaRef>"),
    ]
    .concat();
    let a1_items = item(
        "Sm2TiAudioClip",
        "i-a1",
        "Shot A.mov",
        108000,
        100,
        &format!("{}<MediaTrackIdx>0</MediaTrackIdx>", media_ref("m-video", "R:\\Assets\\Shot A.mov", Some(10))),
    );
    let a2_items = [
        item("Sm2TiAudioClip", "i-a2", "Music.wav", 108000, 290, "<MediaRef>m-audio</MediaRef><MediaTrackIdx>0</MediaTrackIdx>"),
        item("Sm2TiAudioClip", "i-a3", "Music.wav", 108290, 10, "<In>300</In><MediaRef>m-audio</MediaRef>"),
        item("Sm2TiTransition", "i-x2", "Transition", 108284, 12, "<PrettyType>Cross Fade 0DB</PrettyType><AlignmentType>2</AlignmentType>"),
    ]
    .concat();
    let st_items = [
        item(
            "Sm2TiGenerator",
            "i-s1",
            " Hello",
            108000,
            24,
            &format!("<PrettyType>Subtitle</PrettyType><FieldsBlob>{}</FieldsBlob>", subtitle_fields("Hello")),
        ),
        item(
            "Sm2TiGenerator",
            "i-s2",
            " World",
            108020,
            24,
            &format!("<PrettyType>Subtitle</PrettyType><FieldsBlob>{}</FieldsBlob>", subtitle_fields("World")),
        ),
        item("Sm2TiGenerator", "i-s3", " Lost", 108050, 24, "<PrettyType>Subtitle</PrettyType><FieldsBlob/>"),
    ]
    .concat();
    let c_main = format!(
        "{HEAD}<Sm2SequenceContainer DbId=\"c-main\"><VideoTrackVec>{}{}</VideoTrackVec><AudioTrackVec>{}{}</AudioTrackVec><SubtitleTrackVec>{}</SubtitleTrackVec></Sm2SequenceContainer>",
        track("t-v1", 0, 0, "", "s-main", &v1_items),
        track("t-v2", 0, 2, "FX", "s-main", &v2_items),
        track("t-a1", 1, 0, "", "s-main", &a1_items),
        track("t-a2", 1, 2, "MUSIC", "s-main", &a2_items),
        track("t-st", 2, 0, "", "s-main", &st_items),
    );
    let c_nested = format!(
        "{HEAD}<Sm2SequenceContainer DbId=\"c-nested\"><VideoTrackVec>{}</VideoTrackVec><AudioTrackVec/><SubtitleTrackVec/></Sm2SequenceContainer>",
        track(
            "t-n1",
            0,
            0,
            "",
            "s-nested",
            &[
                item("Sm2TiVideoClip", "i-n1", "Shot A.mov", 108000, 200, &media_ref("m-video", "R:\\Assets\\Shot A.mov", None)),
                item("Sm2TiVideoClip", "i-n2", "Main", 108200, 10, "<MediaRef>m-main</MediaRef><MediaStartTime>3600</MediaStartTime>"),
            ]
            .concat()
        ),
    );
    let c_comp = format!(
        "{HEAD}<Sm2SequenceContainer DbId=\"c-comp\"><VideoTrackVec>{}</VideoTrackVec><AudioTrackVec/><SubtitleTrackVec/></Sm2SequenceContainer>",
        track("t-c1", 0, 0, "", "s-comp", &item("Sm2TiVideoClip", "i-c1", "Shot A.mov", 0, 120, &media_ref("m-video", "R:\\Assets\\Shot A.mov", None))),
    );
    write_stored(&[
        ("project.xml", project.as_bytes()),
        ("MediaPool/Master/MpFolder.xml", master.as_bytes()),
        ("MediaPool/Master/000_Sub/MpFolder.xml", sub.as_bytes()),
        ("SeqContainer/c-main.xml", c_main.as_bytes()),
        ("SeqContainer/c-nested.xml", c_nested.as_bytes()),
        ("SeqContainer/c-comp.xml", c_comp.as_bytes()),
        ("Gallery.xml", b"<Gallery::GyGallery DbId=\"g\"/>"),
    ])
}

fn import_synthetic() -> (Imported, Report) {
    import_with(&synthetic(), Format::ResolveProject, &ImportOptions::default()).unwrap()
}

fn seq(p: &Project, id: ItemId) -> &Sequence {
    p.sequence(id).unwrap()
}

fn media_path(p: &Project, id: ItemId) -> Option<&str> {
    match &p.item(id)?.kind {
        ItemKind::Media(m) => match &m.media {
            MediaRef::File { path } => Some(path),
            MediaRef::Generator(_) => None,
        },
        _ => None,
    }
}

fn generator(p: &Project, id: ItemId) -> Option<&Generator> {
    match &p.item(id)?.kind {
        ItemKind::Media(m) => match &m.media {
            MediaRef::Generator(g) => Some(g),
            MediaRef::File { .. } => None,
        },
        _ => None,
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn detected_by_content_and_extension() {
    let bytes = synthetic();
    assert_eq!(detect(&bytes, Some("drp")), Some(Format::ResolveProject));
    assert_eq!(detect(&bytes, None), Some(Format::ResolveProject));
    assert_eq!(Format::from_extension("DRP"), Some(Format::ResolveProject));
    assert_eq!(Format::ResolveProject.extension(), "drp");
    assert_eq!(Format::ALL.len(), 6, "the exportable formats stay as they are");
    assert!(!Format::ALL.contains(&Format::ResolveProject));
    // a ZIP without project.xml is not a Resolve project
    assert_eq!(detect(&write_stored(&[("other.xml", b"<x/>")]), Some("drp")), None);
}

#[test]
fn project_structure() {
    let (imp, report) = import_synthetic();
    let p = &imp.project;
    assert_eq!(p.name, "Synthetic Resolve Project");
    // the Master folder is the root; "Sub" is a bin in it holding the still
    let sub = p.root.children.iter().find_map(|c| match c {
        BinEntry::Bin(b) if b.name == "Sub" => Some(b),
        _ => None,
    });
    let sub = sub.expect("Sub bin");
    assert_eq!(sub.children.len(), 1);
    let still = match sub.children[0] {
        BinEntry::Item(i) => i,
        BinEntry::Bin(_) => panic!("still is an item"),
    };
    assert_eq!(media_path(p, still), Some("R:\\Assets\\Stills\\Still.png"));
    let still_media = p.item(still).unwrap().as_media().unwrap();
    assert_eq!(still_media.info.video.as_ref().map(|v| (v.width, v.height)), Some((640, 480)));
    // two top-level timelines in project order, the compound clip is not one
    assert_eq!(imp.sequences.len(), 2);
    let (main, nested) = (imp.sequences[0], imp.sequences[1]);
    assert_eq!(p.item(main).unwrap().name, "Main");
    assert_eq!(p.item(nested).unwrap().name, "Nested");
    let comp = p.items.values().find(|i| i.name == "Comp" && i.as_sequence().is_some()).expect("compound clip sequence");
    assert!(!imp.sequences.contains(&comp.id));
    // media items keep Resolve's paths; the one without a path is offline
    let shot = p.items.values().find(|i| i.name == "Shot A.mov").unwrap();
    assert_eq!(media_path(p, shot.id), Some("R:\\Assets\\Shot A.mov"));
    let m = shot.as_media().unwrap();
    assert_eq!(m.info.duration, FrameRate::FPS_30.tick_of(300));
    assert_eq!(m.info.audio_streams.len(), 1);
    assert_eq!((m.info.audio_streams[0].sample_rate, m.info.audio_streams[0].channels), (48_000, 2));
    assert!(!m.offline);
    let music = p.items.values().find(|i| i.name == "Music.wav").unwrap().as_media().unwrap();
    assert_eq!(music.info.duration, Tick::from_units(2_400_000, 48_000));
    assert!(media_path(p, p.items.values().find(|i| i.name == "Music.wav").unwrap().id).unwrap().ends_with("Music.wav"));
    let lost = p.items.values().find(|i| i.name == "Lost.mov").unwrap().as_media().unwrap();
    assert!(lost.offline);
    assert!(report.mentions("no file path"));
    for s in p.items.values().filter_map(|i| i.as_sequence()) {
        s.check().unwrap();
    }
}

#[test]
fn main_timeline_tracks() {
    let (imp, report) = import_synthetic();
    let p = &imp.project;
    let main_id = imp.sequences[0];
    let nested_id = imp.sequences[1];
    let main = seq(p, main_id);
    let r = FrameRate::FPS_30;
    assert_eq!(main.settings.frame_rate, r);
    assert_eq!((main.settings.width, main.settings.height), (1920, 1080), "zero Resolution falls back to the media's size");
    assert_eq!(main.start_timecode, 108_000);
    assert_eq!(seq(p, nested_id).settings.width, 1280);
    assert_eq!(main.video_tracks.len(), 2);
    let v1 = &main.video_tracks[0];
    let names: Vec<&str> = v1.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["Shot A.mov", "Nested", "Solid Color", "Title 1", "Lost.mov", "Comp", "Extern.mov"]);
    let shot = &v1.items[0];
    assert_eq!((shot.start, shot.duration, shot.source_in), (Tick::ZERO, r.tick_of(100), r.tick_of(10)));
    let nest = &v1.items[1];
    assert_eq!(nest.item, nested_id, "a timeline clip shows the nested sequence");
    assert_eq!((nest.start, nest.source_in), (r.tick_of(100), r.tick_of(20)));
    assert!(matches!(generator(p, v1.items[2].item), Some(Generator::ColorMatte { .. })));
    assert!(matches!(generator(p, v1.items[3].item), Some(Generator::TransparentVideo)));
    assert!(report.mentions("Title 1") && report.mentions("placeholder"));
    let comp = p.items.values().find(|i| i.name == "Comp").unwrap();
    assert_eq!(v1.items[5].item, comp.id);
    assert_eq!(media_path(p, v1.items[6].item), Some("R:\\Assets\\Extern.mov"));
    assert!(report.mentions("outside the Media Pool"));
    // the overlapping duplicate moved to V2, where the adjustment layer also lives
    let v2 = &main.video_tracks[1];
    assert_eq!(v2.name, "FX");
    assert!(!v2.enabled, "flag 2 disables a video track");
    assert_eq!(v2.items.len(), 3);
    assert_eq!(v2.items[0].start, Tick::ZERO);
    assert!(matches!(p.item(v2.items[1].item).map(|i| &i.kind), Some(ItemKind::AdjustmentLayer { duration, .. }) if *duration == r.tick_of(60)));
    // the Media Pool's own Adjustment Clip generator is an adjustment layer too, as long as its use
    assert_ne!(v2.items[2].item, v2.items[1].item);
    assert!(matches!(p.item(v2.items[2].item).map(|i| &i.kind), Some(ItemKind::AdjustmentLayer { duration, .. }) if *duration == r.tick_of(30)));
    assert_eq!(p.item(v2.items[2].item).unwrap().name, "Adjustment Clip");
    assert!(report.mentions("overlapped"));
    // the dissolve starts at the cut between the shot and the nest
    assert_eq!(v1.transitions.len(), 1);
    let x = &v1.transitions[0];
    assert_eq!((x.from, x.to), (Some(shot.id), Some(nest.id)));
    assert_eq!(x.align, TransitionAlign::StartAtCut);
    assert_eq!((x.start, x.duration), (r.tick_of(100), r.tick_of(12)));
    assert_eq!(x.effect.effect, "cross_dissolve");
    // audio: A1 linked with V1's shot, A2 muted and named, with a centred 0 dB cross fade
    assert_eq!(main.audio_tracks.len(), 2);
    let a1 = &main.audio_tracks[0];
    assert_eq!(a1.items.len(), 1);
    assert!(a1.items[0].link.is_some() && a1.items[0].link == shot.link, "A/V cut from one source are linked");
    assert!(nest.link.is_none());
    let a2 = &main.audio_tracks[1];
    assert_eq!(a2.name, "MUSIC");
    assert!(a2.muted);
    assert_eq!(a2.items.len(), 2);
    assert_eq!(a2.items[1].source_in, r.tick_of(300));
    assert_eq!(a2.transitions.len(), 1);
    assert_eq!(a2.transitions[0].align, TransitionAlign::CenterAtCut);
    assert_eq!(a2.transitions[0].effect.effect, "constant_gain");
    // subtitles: two captions, the first shortened to meet the second, the unreadable one skipped
    assert_eq!(main.caption_tracks.len(), 1);
    let ct = &main.caption_tracks[0];
    assert_eq!(ct.captions.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), ["Hello", "World"]);
    assert_eq!(ct.captions[0].duration, r.tick_of(20));
    assert_eq!(ct.captions[1].start, r.tick_of(20));
    assert!(report.mentions("shortened") && report.mentions("no readable text"));
    ct.check().unwrap();
    // grades counted, the self-nesting clip refused
    assert!(report.mentions("grades or effects"));
    assert!(report.mentions("contain itself"));
    let nested = seq(p, nested_id);
    assert_eq!(nested.video_tracks[0].items.len(), 1, "the clip that would nest Main inside Nested is skipped");
    assert_eq!(nested.start_timecode, 108_000);
    main.check().unwrap();
    nested.check().unwrap();
}

#[test]
fn merging_reallocates_every_id_including_captions() {
    let (imp, _) = import_synthetic();
    let caption_count: usize =
        imp.project.items.values().filter_map(|i| i.as_sequence()).map(|s| s.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>()).sum();
    assert!(caption_count >= 2);
    let mut target = Project::new("target");
    let existing = support::media(&mut target, "/other.mov", true, false, FrameRate::FPS_25);
    let bin = target.add_bin("Imported", None);
    let seqs = merge_into(&mut target, imp, Some(bin));
    assert_eq!(seqs.len(), 2);
    assert!(target.item(existing).is_some());
    let mut ids: HashSet<u64> = HashSet::new();
    for (id, it) in &target.items {
        assert!(ids.insert(id.0));
        if let ItemKind::Sequence(q) = &it.kind {
            for t in q.all_tracks() {
                assert!(ids.insert(t.id.0));
                for c in &t.items {
                    assert!(ids.insert(c.id.0));
                    assert!(target.item(c.item).is_some(), "every clip's source exists after the merge");
                }
            }
            for ct in &q.caption_tracks {
                assert!(ids.insert(ct.id.0), "caption track ids are reallocated");
                for c in &ct.captions {
                    assert!(ids.insert(c.id.0), "caption ids are reallocated");
                }
            }
        }
    }
    assert!(ids.iter().all(|i| *i < target.next_id));
    let merged_captions: usize =
        target.items.values().filter_map(|i| i.as_sequence()).map(|s| s.caption_tracks.iter().map(|t| t.captions.len()).sum::<usize>()).sum();
    assert_eq!(merged_captions, caption_count);
    assert_eq!(target.root.parent_of(seqs[0]), Some(bin));
}

#[test]
fn export_is_refused_and_hostile_archives_never_panic() {
    let (imp, _) = import_synthetic();
    let e = export(&imp.project, imp.sequences[0], Format::ResolveProject, &Default::default()).unwrap_err();
    assert!(e.to_string().contains("imported, not exported"));
    let bytes = synthetic();
    let opts = ImportOptions::default();
    // every truncation
    for n in (0..bytes.len()).step_by(7) {
        let _ = import_with(&bytes[..n], Format::ResolveProject, &opts);
    }
    // bit flips through the archive (central directory, XML text, hex blobs)
    for i in (0..bytes.len()).step_by(5) {
        let mut m = bytes.clone();
        m[i] ^= 0x10;
        if let Ok((imp, _)) = import_with(&m, Format::ResolveProject, &opts) {
            for s in imp.project.items.values().filter_map(|i| i.as_sequence()) {
                s.check().unwrap();
            }
        }
    }
    // an export with nothing in it
    let empty = write_stored(&[("project.xml", b"<SM_Project DbId=\"p\"><ProjectName>Empty</ProjectName></SM_Project>")]);
    assert!(import_with(&empty, Format::ResolveProject, &opts).is_err());
    // a folder document that is not XML is reported, not fatal
    let broken = write_stored(&[
        ("project.xml", b"<SM_Project DbId=\"p\"><ProjectName>Broken</ProjectName></SM_Project>"),
        ("MediaPool/Master/MpFolder.xml", b"<Sm2MpFolder DbId=\"f\"><Name>Master</Name><MediaVec><Element><Sm2MpVideoClip DbId=\"m\"><Name>a.mov</Name></Sm2MpVideoClip></Element></MediaVec></Sm2MpFolder>"),
        ("MediaPool/Master/001_Bad/MpFolder.xml", b"<<not xml"),
    ]);
    let (imp, report) = import_with(&broken, Format::ResolveProject, &opts).unwrap();
    assert!(report.mentions("folder skipped"));
    assert_eq!(imp.project.items.len(), 1);
}

/// Real exports: `FILMCRAFT_RESOLVE_DRP=path.drp;other.drp cargo test -p filmcraft-interchange resolve_real -- --nocapture`.
#[test]
fn resolve_real_exports() {
    let Ok(list) = std::env::var("FILMCRAFT_RESOLVE_DRP") else { return };
    for path in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(detect(&bytes, Some("drp")), Some(Format::ResolveProject), "{path}");
        let t0 = std::time::Instant::now();
        let (imp, report) = import_with(&bytes, Format::ResolveProject, &ImportOptions::default()).unwrap_or_else(|e| panic!("{path}: {e}"));
        let p = &imp.project;
        let seqs: Vec<_> = p.items.values().filter_map(|i| i.as_sequence().map(|s| (i.name.clone(), s))).collect();
        let media = p.items.values().filter(|i| i.as_media().is_some()).count();
        let offline = p.items.values().filter(|i| i.as_media().is_some_and(|m| m.offline)).count();
        let clips: usize = seqs.iter().map(|(_, s)| s.all_tracks().map(|t| t.items.len()).sum::<usize>()).sum();
        println!(
            "{path}: project \"{}\": {} sequences ({} top-level), {media} media ({offline} offline), {clips} clips in {:?}",
            p.name,
            seqs.len(),
            imp.sequences.len(),
            t0.elapsed()
        );
        for id in &imp.sequences {
            let s = p.sequence(*id).unwrap();
            println!(
                "  timeline \"{}\": {}x{} {} start {} V{} A{} captions {} clips {}",
                p.item(*id).unwrap().name,
                s.settings.width,
                s.settings.height,
                s.settings.frame_rate.label(),
                s.start_timecode,
                s.video_tracks.len(),
                s.audio_tracks.len(),
                s.caption_tracks.len(),
                s.all_tracks().map(|t| t.items.len()).sum::<usize>()
            );
        }
        print!("{report}");
        assert!(!imp.sequences.is_empty(), "{path}: no timeline");
        for (name, s) in &seqs {
            s.check().unwrap_or_else(|e| panic!("{path}: {name}: {e}"));
            for t in s.all_tracks() {
                for c in &t.items {
                    assert!(p.item(c.item).is_some(), "{path}: {name}: clip {:?} has no source", c.id);
                }
            }
        }
    }
}
