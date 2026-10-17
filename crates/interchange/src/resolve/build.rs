//! From a parsed [`Drp`] to a FilmCraft project fragment: Media Pool folders become bins, clips
//! become media items (with their file paths), timelines and compound clips become sequences
//! whose tracks hold the clips, transitions, A/V links and subtitles. What Resolve has and this
//! importer does not reproduce (colour grades and effects, Fusion compositions, retimes, the
//! colour of a Solid Color) is counted in the report, never an error.
//!
//! Frame math: Resolve stores `Start`, `Duration` and `In` as frame counts; they become ticks
//! through `FrameRate::tick_of`, so every edit lands exactly on a frame. The timeline start
//! (`MediaExtents`, seconds) is the one floating-point value turned into frames.

use std::collections::{HashMap, HashSet};

use filmcraft_media::{Generator, MediaKind};
use filmcraft_project::{
    AudioChannels, BinId, Caption, CaptionFormat, CaptionTrack, ClipId, ItemId, ItemKind, Sequence, SequenceSettings, TrackId, TrackItem, TrackKind,
    Transition, TransitionAlign, TransitionId,
};
use filmcraft_time::{FrameRate, Tick};

use super::model::{Drp, Folder, Item, ItemKind as ResolveKind, Media, MediaClass, TimelineDef, TrackDef};
use crate::common::{Builder, MediaSpec, empty_sequence, file_name, kind_from_ext, rate_from_f64, settings_for, transition_effect};
use crate::{Error, ImportOptions, Imported, Report, Result};

/// Largest frame count turned into ticks (about 35 years at 1000 fps); beyond it a value is
/// garbage from a damaged file.
const MAX_FRAMES: i64 = 1 << 40;

/// A reserved sequence, filled with tracks once every sequence has an id.
struct Seq {
    item: ItemId,
    def: TimelineDef,
    name: String,
    settings: SequenceSettings,
    /// Timeline frame of the sequence's first frame (`MediaExtents` start × fps).
    start_frames: i64,
    /// A timeline (listed in the project) rather than a compound or multicam clip.
    is_timeline: bool,
}

#[derive(Default)]
struct Counts {
    effects: u32,
    retimed: u32,
    displaced: u32,
    no_media: u32,
    not_in_pool: u32,
    before_start: u32,
    mixed_rate: u32,
    offline: u32,
    default_size: u32,
    untitled_subtitles: u32,
    overlapping_subtitles: u32,
    audio_generators: u32,
}

struct Build<'a> {
    b: Builder,
    report: &'a mut Report,
    /// Fallback picture size: the most common size among the project's video media.
    default_size: (u32, u32),
    /// Media Pool item id → media item.
    media_items: HashMap<String, ItemId>,
    /// Media Pool item id → sequence item (timelines, compound and multicam clips).
    seq_items: HashMap<String, ItemId>,
    seq_rate: HashMap<ItemId, FrameRate>,
    /// Sequence → sequences it nests (to refuse cycles before they exist).
    nests: HashMap<ItemId, HashSet<ItemId>>,
    counts: Counts,
}

/// Build the fragment.
pub(crate) fn build(drp: &Drp, _opts: &ImportOptions, report: &mut Report) -> Result<Imported> {
    let name = if drp.project.name.trim().is_empty() { "Resolve Project" } else { drp.project.name.trim() };
    let mut bx = Build {
        b: Builder::new(name),
        report,
        default_size: default_size(&drp.folders),
        media_items: HashMap::new(),
        seq_items: HashMap::new(),
        seq_rate: HashMap::new(),
        nests: HashMap::new(),
        counts: Counts::default(),
    };
    let bins = bx.bins(&drp.folders);
    // Sequences first, so a clip of a nested timeline finds its item whatever the order.
    let mut seqs: Vec<Seq> = Vec::new();
    for f in &drp.folders {
        let bin = bins.get(&f.id).copied().flatten();
        for m in &f.media {
            if let Some(def) = &m.timeline {
                seqs.push(bx.reserve(m, def, bin));
            }
        }
    }
    for f in &drp.folders {
        let bin = bins.get(&f.id).copied().flatten();
        for m in &f.media {
            if m.timeline.is_none() {
                bx.media(m, bin);
            }
        }
    }
    for s in &seqs {
        bx.fill(s, drp);
    }
    // Top level: the project's timeline order, the current timeline first (it is what opens).
    let by_timeline: HashMap<&str, ItemId> = seqs.iter().filter_map(|s| s.def.timeline_id.as_deref().map(|t| (t, s.item))).collect();
    let mut top: Vec<ItemId> = Vec::new();
    for h in &drp.project.timeline_handles {
        if let Some(&id) = by_timeline.get(h.as_str())
            && !top.contains(&id)
        {
            top.push(id);
        }
    }
    let unlisted: Vec<ItemId> = seqs.iter().filter(|s| s.is_timeline && !top.contains(&s.item)).map(|s| s.item).collect();
    top.extend(unlisted);
    if let Some(i) = drp.project.current_index
        && i < top.len()
        && i > 0
    {
        let cur = top.remove(i);
        top.insert(0, cur);
    }
    bx.b.top = top;
    bx.summarise();
    if bx.b.p.items.is_empty() && bx.b.p.root.children.is_empty() {
        return Err(Error::Other("the project has no media and no timelines".into()));
    }
    Ok(bx.b.finish())
}

/// The most common picture size among the video media (1920 x 1080 when there is none).
fn default_size(folders: &[Folder]) -> (u32, u32) {
    let mut counts: HashMap<(u32, u32), usize> = HashMap::new();
    for m in folders.iter().flat_map(|f| &f.media) {
        if let Some(v) = m.video
            && let (Some(w), Some(h)) = (v.width, v.height)
            && filmcraft_project::validate_frame_size(w, h).is_ok()
        {
            *counts.entry((w, h)).or_default() += 1;
        }
    }
    counts.into_iter().max_by_key(|(size, n)| (*n, std::cmp::Reverse(*size))).map(|(size, _)| size).unwrap_or((1920, 1080))
}

impl Build<'_> {
    /// Folders → bins. The first folder without a parent (Master) is the fragment root itself.
    fn bins(&mut self, folders: &[Folder]) -> HashMap<String, Option<BinId>> {
        let ids: HashSet<&str> = folders.iter().map(|f| f.id.as_str()).collect();
        let mut map: HashMap<String, Option<BinId>> = HashMap::new();
        let mut root_seen = false;
        let mut pending: Vec<&Folder> = Vec::new();
        for f in folders {
            match f.parent.as_deref().filter(|p| ids.contains(p) && *p != f.id) {
                None if !root_seen => {
                    root_seen = true;
                    map.insert(f.id.clone(), None);
                }
                None => {
                    let b = self.b.bin(&f.name, None);
                    map.insert(f.id.clone(), Some(b));
                }
                Some(_) => pending.push(f),
            }
        }
        // Children after their parents, however the archive orders them.
        loop {
            let mut rest = Vec::new();
            let before = pending.len();
            for f in pending {
                match f.parent.as_deref().and_then(|p| map.get(p)).copied() {
                    Some(parent) => {
                        let b = self.b.bin(&f.name, parent);
                        map.insert(f.id.clone(), Some(b));
                    }
                    None => rest.push(f),
                }
            }
            pending = rest;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        for f in pending {
            self.report.info(format!("folder \"{}\" has a parent loop; placed at the top level", f.name));
            let b = self.b.bin(&f.name, None);
            map.insert(f.id.clone(), Some(b));
        }
        map
    }

    fn reserve(&mut self, m: &Media, def: &TimelineDef, bin: Option<BinId>) -> Seq {
        let name = if m.name.trim().is_empty() { "Timeline".to_string() } else { m.name.trim().to_string() };
        let rate = match def.fps {
            Some(f) => rate_from_f64(f),
            None => {
                self.report.info(format!("timeline \"{name}\" states no frame rate; 24 fps assumed"));
                FrameRate::FPS_24
            }
        };
        let (w, h) = match def.size {
            Some((w, h)) => (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0)),
            None => {
                self.counts.default_size += 1;
                self.default_size
            }
        };
        let mut settings = settings_for(rate, w, h, false);
        if settings.validate().is_err() {
            self.report.warn(format!("timeline \"{name}\": its settings are out of range; the project's media size and rate were used"));
            settings = settings_for(rate, self.default_size.0, self.default_size.1, false);
            if settings.validate().is_err() {
                settings = settings_for(FrameRate::FPS_24, 1920, 1080, false);
            }
        }
        settings.audio_master = match def.out_channels {
            Some(1) => AudioChannels::Mono,
            Some(6) => AudioChannels::Surround51,
            _ => AudioChannels::Stereo,
        };
        let start_frames = def
            .start_s
            .map(|s| (s * settings.frame_rate.as_f64()).round())
            .filter(|f| f.is_finite() && f.abs() < MAX_FRAMES as f64)
            .map(|f| f as i64)
            .unwrap_or(0);
        let item = self.b.reserve_sequence(&name, settings.clone(), bin);
        self.seq_items.insert(m.id.clone(), item);
        self.seq_rate.insert(item, settings.frame_rate);
        Seq { item, def: def.clone(), name, settings, start_frames, is_timeline: def.timeline_id.is_some() || m.class == Some(MediaClass::Timeline) }
    }

    /// A Media Pool clip (not a timeline) → media item.
    fn media(&mut self, m: &Media, bin: Option<BinId>) {
        let name = if m.name.trim().is_empty() { m.path.as_deref().map(file_name).unwrap_or("Clip").to_string() } else { m.name.trim().to_string() };
        let (dw, dh) = self.default_size;
        match m.class {
            Some(MediaClass::Generator) if name.to_ascii_lowercase().contains("adjustment") => {
                let id = self.b.adjustment_layer(&format!("adjustment:{}", m.id), &name, dw, dh, FrameRate::FPS_24, Tick::ZERO, bin);
                self.media_items.insert(m.id.clone(), id);
            }
            Some(MediaClass::Generator) => {
                let g = pool_generator(&name, self.report);
                let spec = MediaSpec { video: Some((dw, dh, FrameRate::FPS_24)), ..Default::default() };
                let id = self.b.generator_media(&format!("gen:{}", m.id), &name, g, &spec, bin);
                self.media_items.insert(m.id.clone(), id);
            }
            Some(MediaClass::Fusion) => {
                self.report.warn(format!("Fusion clip \"{name}\" is imported as a transparent placeholder (Fusion compositions are not supported)"));
                let spec = MediaSpec { video: Some((dw, dh, FrameRate::FPS_24)), ..Default::default() };
                let id = self.b.generator_media(&format!("fusion:{}", m.id), &name, Generator::TransparentVideo, &spec, bin);
                self.media_items.insert(m.id.clone(), id);
            }
            _ => self.file_media(m, &name, bin),
        }
    }

    fn file_media(&mut self, m: &Media, name: &str, bin: Option<BinId>) {
        let (dw, dh) = self.default_size;
        let rate = m.video.and_then(|v| v.fps).map(rate_from_f64);
        let video = (m.class == Some(MediaClass::Video) || m.video.is_some()).then(|| {
            let v = m.video.unwrap_or_default();
            (v.width.unwrap_or(dw), v.height.unwrap_or(dh), rate.unwrap_or(FrameRate::FPS_24))
        });
        let audio = m.audio.first().map(|a| (a.sample_rate.unwrap_or(48_000), a.channels.unwrap_or(2)));
        let duration = m.video.and_then(|v| v.num_frames).zip(rate).filter(|(n, _)| *n > 0).map(|(n, r)| r.tick_of(n.min(MAX_FRAMES))).or_else(|| {
            m.audio
                .first()
                .and_then(|a| a.samples.zip(a.sample_rate))
                .filter(|(s, sr)| *s > 0 && *sr > 0)
                .map(|(s, sr)| Tick::from_units(s.min(MAX_FRAMES), i64::from(sr)))
        });
        let start_tc = m.video.and_then(|v| v.start_frame).filter(|f| *f > 0);
        let kind = match (&m.path, &m.class) {
            (Some(p), _) => Some(kind_from_ext(p)),
            (None, Some(MediaClass::Audio)) => Some(MediaKind::AudioOnly),
            _ => None,
        };
        let spec = MediaSpec { duration, video, audio, start_tc, kind };
        let media_rate = rate.unwrap_or(FrameRate::FPS_24);
        let (key, path, offline) = match &m.path {
            Some(p) => (p.clone(), p.clone(), false),
            None => (format!("offline:{}", m.id), name.to_string(), true),
        };
        let fresh = self.b.find_media(&key).is_none();
        let id = self.b.file_media(&key, name, &path, &spec, bin);
        self.media_items.insert(m.id.clone(), id);
        if fresh && let Some(mc) = self.b.p.item_mut(id).and_then(|i| i.as_media_mut()) {
            mc.offline = offline;
            mc.mark_in = m.mark_in.filter(|f| (0..=MAX_FRAMES).contains(f)).map(|f| media_rate.tick_of(f));
            mc.mark_out = m.mark_out.filter(|f| (0..=MAX_FRAMES).contains(f)).map(|f| media_rate.tick_of(f));
            if mc.mark_in.zip(mc.mark_out).is_some_and(|(i, o)| o <= i) {
                mc.mark_in = None;
                mc.mark_out = None;
            }
        }
        if offline {
            self.counts.offline += 1;
        }
    }

    /// Tracks, items, transitions, links and subtitles of one sequence.
    fn fill(&mut self, s: &Seq, drp: &Drp) {
        let container = s.def.seq_ref.as_deref().and_then(|r| drp.containers.get(r)).or_else(|| {
            let sid = s.def.sequence_id.as_deref()?;
            drp.containers.values().find(|c| c.video.iter().chain(&c.audio).chain(&c.subtitle).any(|t| t.sequence_id.as_deref() == Some(sid)))
        });
        let mut seq = empty_sequence(s.settings.clone());
        seq.start_timecode = s.start_frames;
        match container {
            None => self.report.warn(format!("timeline \"{}\" has no track data in the export; imported empty", s.name)),
            Some(c) => {
                for (i, t) in c.video.iter().enumerate() {
                    self.track(&mut seq, s, TrackKind::Video, i, t);
                }
                for (i, t) in c.audio.iter().enumerate() {
                    self.track(&mut seq, s, TrackKind::Audio, i, t);
                }
                for (i, t) in c.subtitle.iter().enumerate() {
                    self.subtitle_track(&mut seq, s, i, t);
                }
                self.link(&mut seq);
            }
        }
        if seq.video_tracks.is_empty() {
            self.b.ensure_tracks(&mut seq, TrackKind::Video, 1);
        }
        if seq.audio_tracks.is_empty() {
            self.b.ensure_tracks(&mut seq, TrackKind::Audio, 1);
        }
        self.b.put_sequence(s.item, seq);
    }

    fn track(&mut self, seq: &mut Sequence, s: &Seq, kind: TrackKind, ti: usize, def: &TrackDef) {
        self.b.ensure_tracks(seq, kind, ti + 1);
        if let Some(t) = seq.tracks_mut(kind).get_mut(ti) {
            if let Some(n) = def.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                t.name = n.to_string();
            }
            // Flag bits are read off exported projects: 1 locked, 2 disabled (video) / muted
            // (audio), 4 solo.
            t.locked = def.flags & 1 != 0;
            match kind {
                TrackKind::Video => t.enabled = def.flags & 2 == 0,
                TrackKind::Audio => t.muted = def.flags & 2 != 0,
            }
            t.solo = def.flags & 4 != 0;
        }
        let mut transitions = Vec::new();
        for item in &def.items {
            if item.kind == ResolveKind::Transition {
                transitions.push(item);
            } else if let Some(ti_item) = self.clip(s, kind, item) {
                let placed = self.b.place(seq, kind, ti, ti_item);
                if placed != ti {
                    self.counts.displaced += 1;
                }
            }
        }
        if let Some(t) = seq.tracks_mut(kind).get_mut(ti) {
            t.sort();
        }
        for tr in transitions {
            self.transition(seq, s, kind, ti, tr);
        }
    }

    /// A clip or generator item → track item.
    fn clip(&mut self, s: &Seq, kind: TrackKind, item: &Item) -> Option<TrackItem> {
        let rate = s.settings.frame_rate;
        if item.duration <= 0 || item.duration > MAX_FRAMES {
            self.report.warn(format!("clip \"{}\" has no duration; skipped", item.name));
            return None;
        }
        let rel = item.start.checked_sub(s.start_frames).filter(|r| (0..=MAX_FRAMES).contains(r));
        let Some(rel) = rel else {
            self.counts.before_start += 1;
            return None;
        };
        let start = rate.tick_of(rel);
        let duration = rate.tick_of(item.duration);
        // Titles and adjustment clips dragged from the Effects Library are video clips with a
        // `PrettyType` and no media reference: generators in all but class.
        let no_media = item.media_ref.is_none() && item.media_path.as_deref().is_none_or(|p| p.trim().is_empty());
        let is_generator = item.kind == ResolveKind::Generator || (no_media && item.pretty_type.as_deref().is_some_and(|p| !p.trim().is_empty()));
        let (source, source_in) = if is_generator { self.generator_item(s, kind, item)? } else { self.source_for(s, kind, item)? };
        self.grow_adjustment(source, duration);
        let name = if item.name.trim().is_empty() { self.b.p.item(source).map(|i| i.name.clone()).unwrap_or_default() } else { item.name.trim().to_string() };
        let mut ti = self.b.clip(source, kind, &name, start, duration, source_in);
        if kind == TrackKind::Audio
            && let Some(idx) = item.media_track_idx.and_then(|i| usize::try_from(i).ok())
        {
            let streams = self.b.p.item(source).and_then(|i| i.as_media()).map(|m| m.info.audio_streams.len()).unwrap_or(0);
            if idx < streams {
                ti.audio_stream = idx;
            }
        }
        if item.timemap.is_some_and(|t| t.retimed) {
            self.counts.retimed += 1;
        }
        if item.has_effects {
            self.counts.effects += 1;
        }
        Some(ti)
    }

    /// The item a clip shows and its source in-point.
    fn source_for(&mut self, s: &Seq, kind: TrackKind, item: &Item) -> Option<(ItemId, Tick)> {
        let in_frames = item.in_frames.unwrap_or(0).clamp(0, MAX_FRAMES);
        if let Some(mref) = &item.media_ref {
            if let Some(&nested) = self.seq_items.get(mref) {
                if nested == s.item || self.reaches(nested, s.item) {
                    self.report.warn(format!("timeline \"{}\" would contain itself through \"{}\"; that clip was skipped", s.name, item.name));
                    return None;
                }
                self.nests.entry(s.item).or_default().insert(nested);
                let nested_rate = self.seq_rate.get(&nested).copied().unwrap_or(s.settings.frame_rate);
                return Some((nested, nested_rate.tick_of(in_frames)));
            }
            if let Some(&id) = self.media_items.get(mref) {
                let media_rate = self.media_rate(item, id, s);
                return Some((id, media_rate.tick_of(in_frames)));
            }
        }
        if let Some(path) = item.media_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            // The clip's media is not in the Media Pool (or its record could not be read): link
            // it by the path the clip carries.
            let rate = item.media_fps.map(rate_from_f64).unwrap_or(s.settings.frame_rate);
            let spec = MediaSpec {
                video: (kind == TrackKind::Video).then_some((self.default_size.0, self.default_size.1, rate)),
                audio: (kind == TrackKind::Audio).then_some((48_000, 2)),
                kind: Some(kind_from_ext(path)),
                ..Default::default()
            };
            let id = self.b.file_media(path, file_name(path), path, &spec, None);
            self.counts.not_in_pool += 1;
            return Some((id, rate.tick_of(in_frames)));
        }
        self.counts.no_media += 1;
        None
    }

    /// The rate the clip's `In` is counted at: the item's media rate, else the media's own, else
    /// the timeline's.
    fn media_rate(&mut self, item: &Item, id: ItemId, s: &Seq) -> FrameRate {
        let own = self.b.p.item(id).and_then(|i| i.as_media()).and_then(|m| m.info.video.as_ref()).map(|v| v.frame_rate);
        let rate = item.media_fps.map(rate_from_f64).or(own).unwrap_or(s.settings.frame_rate);
        if rate != s.settings.frame_rate {
            self.counts.mixed_rate += 1;
        }
        rate
    }

    /// Whether sequence `from` already nests `target` (directly or deeper).
    fn reaches(&self, from: ItemId, target: ItemId) -> bool {
        let mut seen: HashSet<ItemId> = HashSet::new();
        let mut todo = vec![from];
        while let Some(id) = todo.pop() {
            if id == target {
                return true;
            }
            if seen.insert(id)
                && let Some(children) = self.nests.get(&id)
            {
                todo.extend(children.iter().copied());
            }
        }
        false
    }

    /// An adjustment layer lasts as long as its longest use.
    fn grow_adjustment(&mut self, id: ItemId, duration: Tick) {
        if let Some(ItemKind::AdjustmentLayer { duration: d, .. }) = self.b.p.item_mut(id).map(|i| &mut i.kind)
            && *d < duration
        {
            *d = duration;
        }
    }

    /// A generator item (`Sm2TiGenerator`, or a clip with a `PrettyType` and no media) → the item
    /// it shows.
    fn generator_item(&mut self, s: &Seq, kind: TrackKind, item: &Item) -> Option<(ItemId, Tick)> {
        if kind == TrackKind::Audio {
            self.counts.audio_generators += 1;
            return None;
        }
        let (w, h, rate) = (s.settings.width, s.settings.height, s.settings.frame_rate);
        let spec = MediaSpec { video: Some((w, h, rate)), ..Default::default() };
        let pretty = item.pretty_type.as_deref().unwrap_or("").trim();
        let lower = pretty.to_ascii_lowercase();
        let id = if lower == "adjustment clip" {
            let key = format!("adjustment:{w}x{h}@{}", rate.label());
            self.b.adjustment_layer(&key, "Adjustment Layer", w, h, rate, Tick::ZERO, None)
        } else if lower == "solid color" {
            self.report.info("Solid Color generators are imported as black colour mattes (Resolve does not export their colour readably)");
            self.b.generator_media("gen:solid-color", "Solid Color", Generator::ColorMatte { color: [0.0, 0.0, 0.0, 1.0] }, &spec, None)
        } else if lower.contains("color bar") || lower.contains("colour bar") {
            self.b.generator_media("gen:color-bars", "SMPTE Color Bar", Generator::BarsAndTone, &spec, None)
        } else if lower == "subtitle" {
            // Subtitles live on subtitle tracks; one on a video track has nothing to show.
            return None;
        } else {
            let what = if pretty.is_empty() { "generator".to_string() } else { pretty.to_string() };
            self.report.warn(format!(
                "title \"{}\" ({what}) is imported as a transparent placeholder; Fusion titles and generators are not supported",
                item.name.trim()
            ));
            let name = if item.name.trim().is_empty() { what } else { item.name.trim().to_string() };
            self.b.generator_media(&format!("gen:placeholder:{}", item.id), &name, Generator::TransparentVideo, &spec, None)
        };
        Some((id, Tick::ZERO))
    }

    fn transition(&mut self, seq: &mut Sequence, s: &Seq, kind: TrackKind, ti: usize, item: &Item) {
        let rate = s.settings.frame_rate;
        if item.duration <= 0 || item.duration > MAX_FRAMES {
            return;
        }
        let Some(rel) = item.start.checked_sub(s.start_frames).filter(|r| (0..=MAX_FRAMES).contains(r)) else { return };
        let (st, en) = (rate.tick_of(rel), rate.tick_of(rel.saturating_add(item.duration)));
        let name = item.pretty_type.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Cross Dissolve");
        let effect = transition_effect(name, kind == TrackKind::Audio, self.report);
        let Some(track) = seq.tracks_mut(kind).get_mut(ti) else { return };
        let mut from = track.items.iter().filter(|i| i.end() >= st && i.end() <= en).max_by_key(|i| i.end()).map(|i| (i.id, i.end()));
        let mut to = track.items.iter().filter(|i| i.start >= st && i.start <= en).min_by_key(|i| i.start).map(|i| (i.id, i.start));
        // A cut needs both sides to meet; otherwise the transition is single-sided.
        if let (Some((_, fe)), Some((_, ts))) = (from, to)
            && fe != ts
        {
            if ts == st {
                from = None;
            } else {
                to = None;
            }
        }
        if from.is_none() && to.is_none() {
            self.report.warn(format!("transition \"{name}\" at frame {} has no adjacent clip; skipped", item.start));
            return;
        }
        let cut = from.map(|f| f.1).or(to.map(|t| t.1)).unwrap_or(st);
        let align = match item.alignment {
            Some(1) => TransitionAlign::StartAtCut,
            Some(2) => TransitionAlign::CenterAtCut,
            Some(0) => TransitionAlign::EndAtCut,
            _ if cut == st => TransitionAlign::StartAtCut,
            _ if cut == en => TransitionAlign::EndAtCut,
            _ => TransitionAlign::CenterAtCut,
        };
        let id = TransitionId(self.b.alloc());
        track.transitions.push(Transition { id, effect, start: st, duration: en - st, from: from.map(|f| f.0), to: to.map(|t| t.0), align, reverse: false });
        track.sort();
    }

    /// Link the clips cut together from one source: same item, start and duration, on more than
    /// one track and not all of them video.
    fn link(&mut self, seq: &mut Sequence) {
        let mut groups: HashMap<(ItemId, Tick, Tick), Vec<(ClipId, TrackKind)>> = HashMap::new();
        for t in seq.all_tracks() {
            for ti in &t.items {
                groups.entry((ti.item, ti.start, ti.duration)).or_default().push((ti.id, t.kind));
            }
        }
        let mut groups: Vec<_> = groups.into_values().filter(|g| g.len() >= 2 && g.iter().any(|(_, k)| *k == TrackKind::Audio)).collect();
        groups.sort_by_key(|g| g.iter().map(|(c, _)| c.0).min().unwrap_or(0));
        for members in groups {
            let l = self.b.link_id();
            for (c, _) in members {
                if let Some((_, it)) = seq.find_item_mut(c) {
                    it.link = Some(l);
                }
            }
        }
    }

    fn subtitle_track(&mut self, seq: &mut Sequence, s: &Seq, i: usize, def: &TrackDef) {
        let rate = s.settings.frame_rate;
        let name = def.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("Subtitle {}", i + 1));
        let mut ct = CaptionTrack::new(TrackId(self.b.alloc()), name, CaptionFormat::Subtitle);
        ct.locked = def.flags & 1 != 0;
        ct.enabled = def.flags & 2 == 0;
        let mut captions: Vec<Caption> = Vec::new();
        for item in &def.items {
            let Some(text) = item.subtitle_text.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
                self.counts.untitled_subtitles += 1;
                continue;
            };
            if item.duration <= 0 || item.duration > MAX_FRAMES {
                continue;
            }
            let Some(rel) = item.start.checked_sub(s.start_frames).filter(|r| (0..=MAX_FRAMES).contains(r)) else {
                self.counts.before_start += 1;
                continue;
            };
            captions.push(Caption {
                id: ClipId(self.b.alloc()),
                start: rate.tick_of(rel),
                duration: rate.tick_of(item.duration),
                text: text.to_string(),
                speaker: None,
                cue_id: None,
                settings: String::new(),
            });
        }
        captions.sort_by_key(|c| c.start);
        for c in captions {
            if let Some(last) = ct.captions.last_mut()
                && c.start < last.end()
            {
                self.counts.overlapping_subtitles += 1;
                last.duration = c.start - last.start;
                if last.duration.0 <= 0 {
                    ct.captions.pop();
                }
            }
            ct.captions.push(c);
        }
        if !ct.captions.is_empty() {
            seq.caption_tracks.push(ct);
        }
    }

    fn summarise(&mut self) {
        let c = &self.counts;
        let n = |v: u32| format!("{v} clip(s)");
        if c.effects > 0 {
            self.report.warn(format!("{} carry Resolve colour grades or effects, which are not imported", n(c.effects)));
        }
        if c.retimed > 0 {
            self.report.warn(format!("{} may be retimed; speed changes are not imported (clips play at 100%)", n(c.retimed)));
        }
        if c.displaced > 0 {
            self.report.warn(format!("{} overlapped another clip on the same track and were moved to a new track", n(c.displaced)));
        }
        if c.no_media > 0 {
            self.report.warn(format!("{} refer to media the export does not describe; skipped", n(c.no_media)));
        }
        if c.not_in_pool > 0 {
            self.report.info(format!("{} refer to media outside the Media Pool; linked by the path the clip carries", n(c.not_in_pool)));
        }
        if c.before_start > 0 {
            self.report.warn(format!("{} lie before their timeline's start; skipped", n(c.before_start)));
        }
        if c.mixed_rate > 0 {
            self.report
                .info(format!("{} have a frame rate different from their timeline; their source in-points were read at the media rate", n(c.mixed_rate)));
        }
        if c.offline > 0 {
            self.report.warn(format!("{} Media Pool item(s) have no file path in the export; imported offline", c.offline));
        }
        if c.default_size > 0 {
            self.report.info(format!(
                "{} timeline(s) use the project's default resolution; {}x{} (the media's size) was assumed",
                c.default_size, self.default_size.0, self.default_size.1
            ));
        }
        if c.untitled_subtitles > 0 {
            self.report.warn(format!("{} subtitle(s) have no readable text; skipped", c.untitled_subtitles));
        }
        if c.overlapping_subtitles > 0 {
            self.report.warn(format!("{} subtitle(s) overlapped the next one and were shortened", c.overlapping_subtitles));
        }
        if c.audio_generators > 0 {
            self.report.warn(format!("{} generator(s) on audio tracks were skipped", c.audio_generators));
        }
    }
}

/// A Media Pool generator by its name.
fn pool_generator(name: &str, report: &mut Report) -> Generator {
    let lower = name.to_ascii_lowercase();
    if lower.contains("solid") {
        report.info("Solid Color generators are imported as black colour mattes (Resolve does not export their colour readably)");
        Generator::ColorMatte { color: [0.0, 0.0, 0.0, 1.0] }
    } else if lower.contains("color bar") || lower.contains("colour bar") {
        Generator::BarsAndTone
    } else if lower.contains("composition") {
        report.warn(format!("Fusion composition \"{name}\" is imported as transparent video (Fusion compositions are not supported)"));
        Generator::TransparentVideo
    } else {
        report.warn(format!("generator \"{name}\" is not supported; imported as transparent video"));
        Generator::TransparentVideo
    }
}
