//! What a `.drp` says, in Resolve's own terms (nothing from the FilmCraft model yet): the project
//! record, the Media Pool folders with their clips, and the sequence containers with their tracks
//! and items. `build` turns a [`Drp`] into a project fragment. A `Project.db` reader could fill
//! the same structures.
//!
//! Frame positions (`Start`, `Duration`, `In`) are kept as the frame counts Resolve stores;
//! durations in seconds stay seconds. Every field that could not be decoded is simply `None`
//! (or empty); the builder reports what it had to assume.

use std::collections::HashMap;

use roxmltree::Node;

use super::blob::{self, Timemap};
use super::xml::{class, db_id, element_object, element_text, elements_of, object_in, parse_i64, prop_f64, prop_hex, prop_i64, prop_text};

/// Most media pool items, tracks or items one document may hold (each needs a few bytes of XML).
const MAX_OBJECTS: usize = 1 << 20;

/// The parsed export.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Drp {
    pub project: ProjectInfo,
    /// Media Pool folders, in archive order (the root "Master" folder has no parent).
    pub folders: Vec<Folder>,
    /// Sequence containers by their id (`SeqRef` of a sequence).
    pub containers: HashMap<String, Container>,
}

/// `SM_Project`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectInfo {
    pub name: String,
    /// `TimelineHandleVec`: the timelines in the project's order (ids of `Sm2Timeline`).
    pub timeline_handles: Vec<String>,
    /// `CurrentTimelineIndex` into `timeline_handles`.
    pub current_index: Option<usize>,
}

/// `Sm2MpFolder`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folder {
    pub id: String,
    pub name: String,
    /// `MpFolder`: the parent folder's id; `None` for the root.
    pub parent: Option<String>,
    pub media: Vec<Media>,
}

/// The class of a Media Pool item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaClass {
    Video,
    Audio,
    /// A timeline's clip in the Media Pool (`Sm2MpTimelineClip`).
    Timeline,
    Compound,
    Multicam,
    Fusion,
    Generator,
    Other(String),
}

impl MediaClass {
    fn of(class_name: &str) -> MediaClass {
        match class_name {
            "Sm2MpVideoClip" => MediaClass::Video,
            "Sm2MpAudioClip" => MediaClass::Audio,
            "Sm2MpTimelineClip" => MediaClass::Timeline,
            "Sm2MpCompoundClip" => MediaClass::Compound,
            "Sm2MpMulticamClip" => MediaClass::Multicam,
            "Sm2MpFusionClip" => MediaClass::Fusion,
            "Sm2MpGenerator" => MediaClass::Generator,
            other => MediaClass::Other(other.to_string()),
        }
    }
}

/// `BtVideoInfo` (`Time` and `Geometry` maps).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoInfo {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub num_frames: Option<i64>,
    pub start_frame: Option<i64>,
}

/// One `BtAudioTrack` of a `BtAudioInfo`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioInfo {
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// Length in samples.
    pub samples: Option<i64>,
}

/// `Sm2Timeline` + `Sm2Sequence` of a timeline, compound or multicam clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineDef {
    /// `Sm2Timeline` id (what `TimelineHandleVec` lists); compound clips have none.
    pub timeline_id: Option<String>,
    /// `Sm2Sequence` id (what tracks name in `Sequence`).
    pub sequence_id: Option<String>,
    /// `SeqRef`: the sequence container holding the tracks.
    pub seq_ref: Option<String>,
    pub fps: Option<f64>,
    /// `Resolution` (zeros = project default).
    pub size: Option<(i64, i64)>,
    /// `MediaExtents`: start and duration in seconds.
    pub start_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub out_channels: Option<i64>,
}

/// A Media Pool item (`Sm2Mp*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub id: String,
    pub class: Option<MediaClass>,
    pub name: String,
    /// The media file, from the `Clip` record of its video or first audio stream.
    pub path: Option<String>,
    pub video: Option<VideoInfo>,
    pub audio: Vec<AudioInfo>,
    /// `MarkIn` / `MarkOut` in frames.
    pub mark_in: Option<i64>,
    pub mark_out: Option<i64>,
    pub timeline: Option<TimelineDef>,
}

/// `Sm2SequenceContainer`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Container {
    pub id: String,
    pub video: Vec<TrackDef>,
    pub audio: Vec<TrackDef>,
    pub subtitle: Vec<TrackDef>,
}

/// `Sm2TiTrack`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackDef {
    pub id: String,
    /// `Flags` (bit meanings are heuristics: 1 locked, 2 disabled / muted, 4 solo).
    pub flags: i64,
    pub subtype: i64,
    /// `UserDefinedName`.
    pub name: Option<String>,
    /// The `Sm2Sequence` this track belongs to.
    pub sequence_id: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Clip,
    Generator,
    Transition,
}

/// `Sm2TiItem` (`Sm2TiVideoClip`, `Sm2TiAudioClip`, `Sm2TiGenerator`, `Sm2TiTransition`).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub kind: ItemKind,
    pub name: String,
    /// `Start`: timeline frame (including the timeline's start offset).
    pub start: i64,
    /// `Duration` in timeline frames.
    pub duration: i64,
    /// `In`: source frame from the media start (`None` = 0).
    pub in_frames: Option<i64>,
    pub media_ref: Option<String>,
    pub media_path: Option<String>,
    /// `MediaStartTime` in seconds.
    pub media_start_s: Option<f64>,
    pub media_fps: Option<f64>,
    pub media_track_idx: Option<i64>,
    pub link_sync: Option<String>,
    pub pretty_type: Option<String>,
    /// Transitions: `AlignmentType` (1 start at cut, 2 centred, else end at cut).
    pub alignment: Option<i64>,
    /// Subtitle generators: the text.
    pub subtitle_text: Option<String>,
    pub timemap: Option<Timemap>,
    /// `EffectFiltersBA` is present (grades / effects that are not imported).
    pub has_effects: bool,
}

/// Parse `project.xml`.
pub fn parse_project(doc: &roxmltree::Document<'_>) -> Result<ProjectInfo, String> {
    let root = doc.root_element();
    if class(root) != "SM_Project" {
        return Err(format!("project.xml: root element is <{}>, not <SM_Project>", class(root)));
    }
    let name = prop_text(root, "ProjectName").unwrap_or("Resolve Project").to_string();
    let timeline_handles = elements_of(root, "TimelineHandleVec").filter_map(element_text).map(str::to_string).take(MAX_OBJECTS).collect();
    let current_index = prop_i64(root, "CurrentTimelineIndex").and_then(|i| usize::try_from(i).ok());
    Ok(ProjectInfo { name, timeline_handles, current_index })
}

/// Parse one `MpFolder.xml`.
pub fn parse_folder(doc: &roxmltree::Document<'_>) -> Result<Folder, String> {
    let root = doc.root_element();
    if class(root) != "Sm2MpFolder" {
        return Err(format!("MpFolder.xml: root element is <{}>, not <Sm2MpFolder>", class(root)));
    }
    let id = db_id(root).ok_or("MpFolder.xml: folder has no DbId")?.to_string();
    let name = prop_text(root, "Name").unwrap_or("Folder").to_string();
    let parent = prop_text(root, "MpFolder").map(str::to_string);
    let media = elements_of(root, "MediaVec").filter_map(element_object).map(parse_media).take(MAX_OBJECTS).collect();
    Ok(Folder { id, name, parent, media })
}

fn parse_media(n: Node<'_, '_>) -> Media {
    let mut m = Media {
        id: db_id(n).unwrap_or_default().to_string(),
        class: Some(MediaClass::of(class(n))),
        name: prop_text(n, "Name").unwrap_or("").to_string(),
        mark_in: prop_i64(n, "MarkIn"),
        mark_out: prop_i64(n, "MarkOut"),
        ..Media::default()
    };
    if let Some(v) = object_in(n, "Video") {
        m.path = clip_path(v);
        m.video = Some(video_info(v));
    }
    for a in elements_of(n, "EmbeddedAudioVec").filter_map(element_object).take(64) {
        if m.path.is_none() {
            m.path = clip_path(a);
        }
        m.audio.extend(audio_tracks(a));
    }
    // A timeline clip inlines its timeline; a compound or multicam clip inlines its sequence.
    if let Some(tl) = object_in(n, "TimelineSharedHandle") {
        let mut def = object_in(tl, "Sequence").map(sequence_def).unwrap_or_default();
        def.timeline_id = db_id(tl).map(str::to_string);
        m.timeline = Some(def);
    } else if let Some(seq) = object_in(n, "Sequence") {
        m.timeline = Some(sequence_def(seq));
    }
    m
}

/// The file behind a `BtVideoInfo` / `BtAudioInfo`: its `Clip` record names the directory
/// (field 1) and the file (field 2).
fn clip_path(info: Node<'_, '_>) -> Option<String> {
    let raw = prop_hex(info, "Clip")?;
    let msg = blob::v2_payload(&raw).ok()?;
    let dir = blob::proto_string(&msg, 1)?;
    let file = blob::proto_string(&msg, 2)?;
    if file.is_empty() {
        return None;
    }
    if dir.is_empty() {
        return Some(file);
    }
    let sep = if dir.contains('\\') && !dir.contains('/') { '\\' } else { '/' };
    let dir = dir.trim_end_matches(['/', '\\']);
    Some(format!("{dir}{sep}{file}"))
}

fn video_info(v: Node<'_, '_>) -> VideoInfo {
    let mut out = VideoInfo::default();
    if let Some(time) = prop_hex(v, "Time").and_then(|b| blob::qt_map(&b).ok()) {
        out.num_frames = time.get("NumFrames").and_then(|x| x.as_i64());
        out.start_frame = time.get("StartFrame").and_then(|x| x.as_i64());
        out.fps = time.get("FrameRate").and_then(|x| x.as_bytes()).and_then(blob::f64_pair_le).map(|(fps, _)| fps).filter(|f| *f > 0.0);
    }
    if let Some(geo) = prop_hex(v, "Geometry").and_then(|b| blob::qt_map(&b).ok())
        && let Some((w, h)) = geo.get("Resolution").and_then(|x| x.as_bytes()).and_then(blob::i64_pair_be)
    {
        out.width = u32::try_from(w).ok().filter(|w| *w > 0);
        out.height = u32::try_from(h).ok().filter(|h| *h > 0);
    }
    out
}

/// The `TracksBA` map of a `BtAudioInfo`: one nested map per track (`"0"`, `"1"`, …).
fn audio_tracks(a: Node<'_, '_>) -> Vec<AudioInfo> {
    let Some(tracks) = prop_hex(a, "TracksBA").and_then(|b| blob::qt_map(&b).ok()) else { return Vec::new() };
    let mut keyed: Vec<(i64, AudioInfo)> = tracks
        .iter()
        .filter_map(|(k, v)| {
            let t = blob::qt_map(v.as_bytes()?).ok()?;
            let info = AudioInfo {
                sample_rate: t.get("SampleRate").and_then(|x| x.as_i64()).and_then(|x| u32::try_from(x).ok()).filter(|x| *x > 0),
                channels: t.get("NumChannels").and_then(|x| x.as_i64()).and_then(|x| u32::try_from(x).ok()).filter(|x| *x > 0),
                samples: t.get("Duration").and_then(|x| x.as_i64()).filter(|x| *x >= 0),
            };
            Some((k.parse::<i64>().unwrap_or(i64::MAX), info))
        })
        .collect();
    keyed.sort_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, i)| i).collect()
}

fn sequence_def(seq: Node<'_, '_>) -> TimelineDef {
    let fields = prop_hex(seq, "FieldsBlob").and_then(|b| blob::qt_map(&b).ok());
    let extents = prop_hex(seq, "MediaExtents").and_then(|b| blob::f64_pair_le(&b));
    TimelineDef {
        timeline_id: None,
        sequence_id: db_id(seq).map(str::to_string),
        seq_ref: fields.as_ref().and_then(|f| f.get("SeqRef")).and_then(|v| v.as_str()).map(str::to_string),
        fps: prop_hex(seq, "FrameRate").and_then(|b| blob::f64_pair_le(&b)).map(|(fps, _)| fps).filter(|f| *f > 0.0),
        size: prop_hex(seq, "Resolution").and_then(|b| blob::i64_pair_be(&b)).filter(|(w, h)| *w > 0 && *h > 0),
        start_s: extents.map(|(s, _)| s).filter(|s| s.is_finite() && *s >= 0.0),
        duration_s: extents.map(|(_, d)| d).filter(|d| d.is_finite() && *d >= 0.0),
        out_channels: prop_i64(seq, "NumOutputAudioChannels"),
    }
}

/// Parse one `SeqContainer/<uuid>.xml`.
pub fn parse_container(doc: &roxmltree::Document<'_>) -> Result<Container, String> {
    let root = doc.root_element();
    if class(root) != "Sm2SequenceContainer" {
        return Err(format!("sequence container: root element is <{}>, not <Sm2SequenceContainer>", class(root)));
    }
    let tracks = |prop: &str| elements_of(root, prop).filter_map(element_object).map(parse_track).take(MAX_OBJECTS).collect::<Vec<_>>();
    Ok(Container {
        id: db_id(root).ok_or("sequence container has no DbId")?.to_string(),
        video: tracks("VideoTrackVec"),
        audio: tracks("AudioTrackVec"),
        subtitle: tracks("SubtitleTrackVec"),
    })
}

fn parse_track(t: Node<'_, '_>) -> TrackDef {
    TrackDef {
        id: db_id(t).unwrap_or_default().to_string(),
        flags: prop_i64(t, "Flags").unwrap_or(0),
        subtype: prop_i64(t, "SubType").unwrap_or(0),
        name: prop_text(t, "UserDefinedName").map(str::to_string),
        sequence_id: prop_text(t, "Sequence").map(str::to_string),
        items: elements_of(t, "Items").filter_map(element_object).filter_map(parse_item).take(MAX_OBJECTS).collect(),
    }
}

fn parse_item(n: Node<'_, '_>) -> Option<Item> {
    let kind = match class(n) {
        "Sm2TiTransition" => ItemKind::Transition,
        "Sm2TiGenerator" => ItemKind::Generator,
        _ => ItemKind::Clip,
    };
    let start = prop_text(n, "Start").and_then(parse_i64)?;
    let duration = prop_text(n, "Duration").and_then(parse_i64)?;
    let pretty_type = prop_text(n, "PrettyType").map(str::to_string);
    let subtitle_text = (kind == ItemKind::Generator && pretty_type.as_deref() == Some("Subtitle"))
        .then(|| {
            let raw = prop_hex(n, "FieldsBlob")?;
            let msg = blob::v2_payload(&raw).ok()?;
            blob::proto_string(blob::proto_bytes(&msg, 1)?, 14)
        })
        .flatten();
    Some(Item {
        id: db_id(n).unwrap_or_default().to_string(),
        kind,
        name: prop_text(n, "Name").unwrap_or("").to_string(),
        start,
        duration,
        in_frames: prop_i64(n, "In"),
        media_ref: prop_text(n, "MediaRef").map(str::to_string),
        media_path: prop_text(n, "MediaFilePath").map(str::to_string),
        media_start_s: prop_f64(n, "MediaStartTime"),
        media_fps: prop_hex(n, "MediaFrameRate").and_then(|b| blob::f64_pair_le(&b)).map(|(fps, _)| fps).filter(|f| *f > 0.0),
        media_track_idx: prop_i64(n, "MediaTrackIdx"),
        link_sync: prop_text(n, "LinkedItemSync").map(str::to_string),
        pretty_type,
        alignment: prop_i64(n, "AlignmentType"),
        subtitle_text,
        timemap: prop_hex(n, "MediaTimemapBA").and_then(|b| blob::timemap(&b)),
        has_effects: prop_text(n, "EffectFiltersBA").is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::xml;
    use super::*;

    fn doc(s: &str) -> String {
        xml::sanitise_tags(s)
    }

    #[test]
    fn project_record() {
        let s = doc(
            "<SM_Project DbId=\"p\"><ProjectName>Waste World News</ProjectName><TimelineHandleVec><Element>t1</Element><Element>t2</Element></TimelineHandleVec>\
                     <CurrentTimelineIndex>1</CurrentTimelineIndex><PowerNodeList><ListMgt::LmPowerNodeList DbId=\"x\"/></PowerNodeList></SM_Project>",
        );
        let d = xml::parse(&s).unwrap();
        let p = parse_project(&d).unwrap();
        assert_eq!(p.name, "Waste World News");
        assert_eq!(p.timeline_handles, vec!["t1", "t2"]);
        assert_eq!(p.current_index, Some(1));
        let d = xml::parse("<Other/>").unwrap();
        assert!(parse_project(&d).is_err());
    }

    #[test]
    fn folder_with_clips_and_a_timeline() {
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
        fn bytes_var(payload: &[u8]) -> Vec<u8> {
            let mut v = (payload.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(payload);
            var(12, &v)
        }
        fn qmap_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend((entries.len() as u32).to_be_bytes());
            for (k, val) in entries {
                v.extend(qs(k));
                v.extend(val);
            }
            v
        }
        fn f64_pair(a: f64, b: f64) -> Vec<u8> {
            let mut v = a.to_le_bytes().to_vec();
            v.extend(b.to_le_bytes());
            v
        }
        fn i64_pair(a: i64, b: i64) -> Vec<u8> {
            let mut v = a.to_be_bytes().to_vec();
            v.extend(b.to_be_bytes());
            v
        }
        /// A `Clip` record: field 1 directory, field 2 file name, as a raw v2 payload.
        fn clip_record(dir: &str, file: &str) -> String {
            let mut msg = vec![0x0a, dir.len() as u8];
            msg.extend_from_slice(dir.as_bytes());
            msg.push(0x12);
            msg.push(file.len() as u8);
            msg.extend_from_slice(file.as_bytes());
            let mut clip = 2u32.to_be_bytes().to_vec();
            clip.extend((msg.len() as u32 + 1).to_be_bytes());
            clip.push(0x80);
            clip.extend(&msg);
            hexs(&clip)
        }
        let clip_a = clip_record("R:\\x", "a.mov");
        let clip_b = clip_record("R:\\x\\", "b.wav");
        let time = hexs(&qmap_bytes(&[
            ("NumFrames", var(2, &1478i32.to_be_bytes())),
            ("StartFrame", var(2, &0i32.to_be_bytes())),
            ("FrameRate", bytes_var(&f64_pair(30.0, 0.0))),
        ]));
        let geo = hexs(&qmap_bytes(&[("Resolution", bytes_var(&i64_pair(1920, 1080)))]));
        let track0 = qmap_bytes(&[
            ("SampleRate", var(3, &48_000u32.to_be_bytes())),
            ("NumChannels", var(2, &2i32.to_be_bytes())),
            ("Duration", var(4, &2_426_065i64.to_be_bytes())),
        ]);
        let tracks = hexs(&qmap_bytes(&[("0", bytes_var(&track0))]));
        let fields = hexs(&qmap_bytes(&[("SeqRef", var(10, &qs("c1")))]));
        let extents = hexs(&f64_pair(3600.0, 50.5333333333333));
        let fps30 = hexs(&f64_pair(30.0, 0.0));
        let fps24 = hexs(&f64_pair(24.0, 0.0));
        let res = hexs(&i64_pair(1920, 1080));
        let s = doc(&format!(
            "<Sm2MpFolder DbId=\"f\"><Name>Master</Name><MpFolder/><MediaVec>\
             <Element><Sm2MpVideoClip DbId=\"m1\"><Name>a.mov</Name><MarkIn>10</MarkIn><MarkOut>20</MarkOut>\
               <Video><BtVideoInfo DbId=\"v\"><Clip>{clip_a}</Clip><Time>{time}</Time><Geometry>{geo}</Geometry></BtVideoInfo></Video></Sm2MpVideoClip></Element>\
             <Element><Sm2MpTimelineClip DbId=\"m2\"><Name>TEST</Name><TimelineSharedHandle><Sm2Timeline DbId=\"t1\"><Name>TEST</Name>\
               <Sequence><Sm2Sequence DbId=\"s1\"><FieldsBlob>{fields}</FieldsBlob>\
               <MediaExtents>{extents}</MediaExtents><FrameRate>{fps30}</FrameRate>\
               <Resolution>{res}</Resolution><NumOutputAudioChannels>2</NumOutputAudioChannels></Sm2Sequence></Sequence>\
               </Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element>\
             <Element><Sm2MpCompoundClip DbId=\"m3\"><Name>Comp</Name><Sequence><Sm2Sequence DbId=\"s2\"><FrameRate>{fps24}</FrameRate></Sm2Sequence></Sequence></Sm2MpCompoundClip></Element>\
             <Element><Sm2MpFusionClip DbId=\"m4\"><Name>Fx</Name></Sm2MpFusionClip></Element>\
             <Element><Sm2MpAudioClip DbId=\"m5\"><Name>b.wav</Name><EmbeddedAudioVec><Element><BtAudioInfo DbId=\"a\"><Clip>{clip_b}</Clip><TracksBA>{tracks}</TracksBA></BtAudioInfo></Element></EmbeddedAudioVec></Sm2MpAudioClip></Element>\
             </MediaVec></Sm2MpFolder>"
        ));
        let d = xml::parse(&s).unwrap();
        let f = parse_folder(&d).unwrap();
        assert_eq!((f.id.as_str(), f.name.as_str(), f.parent.as_deref()), ("f", "Master", None));
        assert_eq!(f.media.len(), 5);
        let a = &f.media[4];
        assert_eq!((a.class.clone(), a.path.as_deref()), (Some(MediaClass::Audio), Some("R:\\x\\b.wav")));
        assert_eq!(a.audio, vec![AudioInfo { sample_rate: Some(48_000), channels: Some(2), samples: Some(2_426_065) }]);
        let m = &f.media[0];
        assert_eq!(m.class, Some(MediaClass::Video));
        assert_eq!(m.path.as_deref(), Some("R:\\x\\a.mov"));
        assert_eq!((m.mark_in, m.mark_out), (Some(10), Some(20)));
        let v = m.video.unwrap();
        assert_eq!((v.num_frames, v.start_frame, v.fps, v.width, v.height), (Some(1478), Some(0), Some(30.0), Some(1920), Some(1080)));
        let t = f.media[1].timeline.as_ref().unwrap();
        assert_eq!(t.timeline_id.as_deref(), Some("t1"));
        assert_eq!(t.sequence_id.as_deref(), Some("s1"));
        assert_eq!(t.seq_ref.as_deref(), Some("c1"));
        assert_eq!(t.fps, Some(30.0));
        assert_eq!(t.size, Some((1920, 1080)));
        assert_eq!(t.start_s, Some(3600.0));
        assert!((t.duration_s.unwrap() - 50.5333).abs() < 1e-3);
        assert_eq!(t.out_channels, Some(2));
        let c = f.media[2].timeline.as_ref().unwrap();
        assert_eq!((c.timeline_id.as_deref(), c.sequence_id.as_deref(), c.fps), (None, Some("s2"), Some(24.0)));
        assert_eq!(f.media[3].class, Some(MediaClass::Fusion));
        assert!(f.media[3].timeline.is_none() && f.media[3].path.is_none());
    }

    #[test]
    fn container_tracks_and_items() {
        let s = doc(
            "<Sm2SequenceContainer DbId=\"c1\"><VideoTrackVec><Element><Sm2TiTrack DbId=\"v1\"><Type>0</Type><SubType>6029427</SubType><Flags>0</Flags><Sequence>s1</Sequence>\
             <Items><Element><Sm2TiVideoClip DbId=\"i1\"><Name>a.mov</Name><Start>108000</Start><Duration>1478</Duration><In/><MediaRef>m1</MediaRef><MediaStartTime>0</MediaStartTime>\
             <MediaFilePath>R:\\x\\a.mov</MediaFilePath><MediaFrameRate>0000000000003e400000000000000000</MediaFrameRate><MediaTimemapBA>0240489dddddddddde</MediaTimemapBA><EffectFiltersBA/></Sm2TiVideoClip></Element>\
             <Element><Sm2TiTransition DbId=\"x1\"><PrettyType>Cross Dissolve</PrettyType><Name>Transition</Name><Start>109000</Start><Duration>24</Duration><AlignmentType>2</AlignmentType></Sm2TiTransition></Element>\
             <Element><Sm2TiGenerator DbId=\"g1\"><PrettyType>Subtitle</PrettyType><Name> Hi</Name><Start>87952</Start><Duration>12</Duration><FieldsBlob>000000020000000a800a0772032048697804</FieldsBlob></Sm2TiGenerator></Element>\
             <Element><Sm2TiVideoClip DbId=\"bad\"><Name>no start</Name><Duration>5</Duration></Sm2TiVideoClip></Element>\
             </Items></Sm2TiTrack></Element></VideoTrackVec>\
             <AudioTrackVec><Element><Sm2TiTrack DbId=\"a1\"><Type>1</Type><SubType>0</SubType><Flags>2</Flags><UserDefinedName>DIALOGUE</UserDefinedName><Sequence>s1</Sequence>\
             <Items><Element><Sm2TiAudioClip DbId=\"i2\"><Name>a.mov</Name><Start>108000</Start><Duration>1478</Duration><In>3</In><MediaRef>m1</MediaRef><MediaTrackIdx>0</MediaTrackIdx><LinkedItemSync>-1</LinkedItemSync><EffectFiltersBA>00</EffectFiltersBA></Sm2TiAudioClip></Element></Items></Sm2TiTrack></Element></AudioTrackVec>\
             <SubtitleTrackVec/></Sm2SequenceContainer>",
        );
        let d = xml::parse(&s).unwrap();
        let c = parse_container(&d).unwrap();
        assert_eq!(c.id, "c1");
        assert_eq!((c.video.len(), c.audio.len(), c.subtitle.len()), (1, 1, 0));
        let v = &c.video[0];
        assert_eq!(v.sequence_id.as_deref(), Some("s1"));
        assert_eq!(v.items.len(), 3, "the item without a Start is skipped");
        let i = &v.items[0];
        assert_eq!(i.kind, ItemKind::Clip);
        assert_eq!((i.start, i.duration, i.in_frames), (108000, 1478, None));
        assert_eq!(i.media_ref.as_deref(), Some("m1"));
        assert_eq!(i.media_path.as_deref(), Some("R:\\x\\a.mov"));
        assert_eq!(i.media_fps, Some(30.0));
        assert!((i.timemap.unwrap().media_seconds - 49.2333).abs() < 1e-3);
        assert!(!i.has_effects, "an empty EffectFiltersBA is no effect");
        let x = &v.items[1];
        assert_eq!((x.kind, x.alignment, x.pretty_type.as_deref()), (ItemKind::Transition, Some(2), Some("Cross Dissolve")));
        let g = &v.items[2];
        assert_eq!((g.kind, g.subtitle_text.as_deref()), (ItemKind::Generator, Some(" Hi")));
        let a = &c.audio[0];
        assert_eq!((a.flags, a.name.as_deref()), (2, Some("DIALOGUE")));
        assert_eq!((a.items[0].in_frames, a.items[0].media_track_idx, a.items[0].link_sync.as_deref()), (Some(3), Some(0), Some("-1")));
        assert!(a.items[0].has_effects);
    }
}
