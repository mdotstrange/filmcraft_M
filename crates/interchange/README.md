# filmcraft-interchange

Timeline interchange for FilmCraft: CMX 3600 EDL, Final Cut Pro 7 XML (xmeml), FCPXML,
OpenTimelineIO, AAF and OMF import and export, plus Avid ALE. Layer L2: no file I/O; media
references are strings and audio essence is supplied by the caller (the engine).

## Specifications

Clean-room: written from public specifications only. The AAF SDK, pyaaf2, the OMF Toolkit and
other implementations were not consulted. Where a specification leaves a layout open (how a
nested sequence is written, how several video tracks share a slot), what Premiere Pro does was
read off files it exports, never from the application itself.

| Format | Document | Edition | Used for |
|---|---|---|---|
| AAF | AMWA *AAF Object Specification* | v1.1 (2005) | classes, properties (ids shared with SMPTE ST 377-1 local tags), mob / slot / segment model, sequences and transitions, operation groups, parameters, definitions, descriptors, locators, essence data |
| AAF | AMWA *AAF Low-Level Container Specification* | v1.0.1 | objects as structured storages, `properties` streams, stored forms, strong / weak reference collections and their index streams, `referenced properties` |
| AAF | AMWA *AAF Edit Protocol* | v1.0 (AMWA AS-01) | top-level composition, mob chains (composition → master → file → physical source), operation definitions (dissolve, fade to black, audio gain / dissolve), usage codes |
| AAF | SMPTE RP 224 / RP 210 registers | — | data definition, class and type labels |
| AAF | Microsoft [MS-CFB] | rev. 10.0 | the container, see [`filmcraft-cfb`](../cfb/README.md) |
| OMF | Avid *OMF Interchange Specification* | Version 2.0 (1997) | classes (`HEAD`, `CMOB`, `MMOB`, `SMOB`, `MSLT`, `TRKD`, `SEQU`, `SCLP`, `FILL`, `TRAN`, `EFFE`, `ESLT`, `CVAL`, `VVAL`, `TCCP`, `WAVD`, `AIFD`, `WAVE`, `AIFC`, locators), property and type names |
| OMF | Apple *Bento Specification* | revision 1.0d5 | container label, TOC encoding, objects / properties / types, references |
| WAVE / AIFF | Microsoft RIFF WAVE; Apple AIFF 1.3 | — | embedded and separate audio files |

The other formats' editions are listed in the module docs (`edl`, `fcp7`, `fcpxml`, `otio`, `ale`).

**Invariant — Unicode media URLs:** file URL prefix probes must not panic when their byte boundary
falls inside a UTF-8 character. Timeline importers use the shared converter; `tests/file_urls.rs`
checks expected Unicode paths through the public converter, resolver and EDL importer.

## AAF and OMF

`comp` holds a format-neutral mob-style model (compositions of slots whose sequences of fillers,
source clips and overlapping transitions reference media through master and file source mobs).
`aaf` and `omf` serialise it; both import back into the same model and from there into FilmCraft
sequences and media. Mapping details (what each FilmCraft feature becomes) are in the `aaf` and
`omf` module docs. Times are kept in ticks in the model; compositions use the sequence frame rate
for picture and the sample rate for sound slots, so audio edits are sample-exact.

Embedded / consolidated audio (`essence`): `audio_needs` lists the media ranges an export
references (per media item or per clip, with handles); the engine decodes or renders them and
passes `AudioEssence` (embedded PCM or a written file) back in `MediaOptions`.

Not represented: speed changes and frame holds (exported at 100 % with a report entry),
graphics and synthetic media (gaps), video effects other than transitions, clip
markers (written to the master mob, so they come back as media markers), OMF video and markers.

## DaVinci Resolve projects

`resolve` imports a whole Resolve project from a `.drp` export (Project Manager ▸ Export
Project); FilmCraft never writes `.drp` and never touches Resolve's own database. Clean-room:
everything was read off files Resolve 21 exports from projects we authored; no Resolve binary,
SDK or database driver was consulted.

**Container and dialect.** A `.drp` is a ZIP (stored or DEFLATE entries; `resolve::zipfile`, on the
EXR inflate decoder) of XML documents, one per database object tree: `project.xml`
(`SM_Project`), `MediaPool/Master/…/MpFolder.xml` (one per Media Pool folder, nested like the
folders; the `<Name>` and `<MpFolder>` parent id are authoritative, not the path),
`SeqContainer/<uuid>.xml` (the tracks and items of one timeline or compound clip) and
`Gallery.xml` (ignored). An object is `<ClassName DbId="uuid">` with one child element per
property, collections are `<Prop><Element>…</Element></Prop>`, an empty element is a null, and
class names may contain `::`, which `resolve::xml` rewrites to `__` inside tags before parsing.

**Binary fields** (`resolve::blob`, hex text in the XML): Qt-serialised maps (`u32 version, u32
count, (UTF-16BE key, QVariant)*`), versioned payloads (`00 00 00 02, u32 length, flag`: `0x80`
a raw protocol-buffers message, `0x81` the same inside a zstd frame, decoded with the pure-Rust
`ruzstd`), qCompress byte arrays (big-endian length + zlib), 16-byte pairs (frame rates and media
extents as little-endian doubles, picture sizes as big-endian 64-bit integers) and
`MediaTimemapBA` (version byte + big-endian doubles). Decompressed sizes are capped at 64 MiB;
every decoder has truncation and bit-flip tests.

| Resolve | FilmCraft |
|---|---|
| Media Pool folder (`Sm2MpFolder`) | bin; the root "Master" folder is the fragment root |
| Video / audio clip (`Sm2MpVideoClip`, `Sm2MpAudioClip`) | media item at the path of its `Clip` record (directory + file); size, frame rate, frame count, sample rate, channels and marks from the `Time`, `Geometry` and `TracksBA` maps; no path → offline item |
| Timeline (`Sm2MpTimelineClip` → `Sm2Timeline` → `Sm2Sequence`) | sequence: frame rate from `FrameRate`, size from `Resolution` (zeros = the most common media size), start timecode from `MediaExtents`, master channels from `NumOutputAudioChannels`; tracks from the `SeqRef` container |
| Compound clip (`Sm2MpCompoundClip`) | sequence (not top-level) |
| Track (`Sm2TiTrack`): `UserDefinedName`, `Flags` | track name; flags 1 locked, 2 disabled (video) / muted (audio), 4 solo (read off exports) |
| Clip (`Sm2TiVideoClip`, `Sm2TiAudioClip`): `Start`, `Duration`, `In`, `MediaRef` | clip at `Start − timeline start` frames, `In` frames of source at the media rate (`MediaFrameRate`); a `MediaRef` to a timeline or compound clip nests that sequence (`In` relative to its start); a clip whose media is not in the pool is linked by its `MediaFilePath` |
| Transition (`Sm2TiTransition`): `PrettyType`, `AlignmentType` | transition between the clips meeting in its range; alignment 1 start at cut, 2 centred, else end at cut; `Cross Fade 0DB` → Constant Gain, unknown names → Cross Dissolve with a warning |
| Generators (`Sm2TiGenerator`): Solid Color, SMPTE Color Bar, Adjustment Clip | black colour matte (the colour is not readable), Bars and Tone, adjustment layer |
| Fusion titles and clips, other generators | transparent placeholder clip named after the item, with a warning |
| Subtitle track items | caption track (`Subtitle`), text from the item's fields; overlaps are shortened |
| Clips of one source cut together on video and audio tracks | linked |
| `TimelineHandleVec` / `CurrentTimelineIndex` | top-level sequence order; the current timeline first (it opens after the import) |

Not imported, counted in the report: colour grades and effects (`EffectFiltersBA`), retimes
(`MediaTimemapBA` beyond the plain map), markers (layout unknown), Fusion compositions, clip
colours, audio track formats and the project-wide settings blob (render settings only).

`cargo run -p filmcraft-interchange --example resolve_dump -- project.drp [entry-filter]` prints an
export's documents with every binary field decoded. `tests/resolve.rs` imports a synthetic export
built in-test and, when `FILMCRAFT_RESOLVE_DRP` lists `.drp` paths (`;`-separated), real ones,
printing their reports.

## Nested sequences

For the formats Premiere Pro exports (FCP7 XML, OTIO, EDL, AAF, OMF) a nested sequence is written
the way Premiere Pro 26.5.2 writes it, as seen in the files it exports for a sequence with a nest.
Premiere Pro does not write FCPXML.

| Format | Written as | On import into FilmCraft |
|---|---|---|
| FCP7 XML | a clip item holding the nested `<sequence>` (by id after the first use) | a sequence the clips use |
| FCPXML | a `ref-clip` to a `media` resource holding the sequence | a sequence the clips use |
| OTIO | a `Stack` inside the track | a sequence the clips use |
| EDL | one event: reel `AX` in every reel mode, the nested sequence's name as the clip name, its own time as source timecode (the report says the nest's edit is not in the EDL) | an ordinary clip of that name |
| AAF | a composition mob of its own (not tagged top-level); the nest's clips are source clips that point at it, at its first track of their kind. With embedded or consolidated audio the media inside the nest is prepared too (`NestNeeds::Inside`) | a sequence the clips use, read once however many clips use it |
| OMF | the nested sequence's sound, mixed by the engine with what is on the clip (`NestNeeds::Render`), as one clip named after the sequence. Without that essence: a gap, and the report names the sequence | media in the document |

A sequence that claims to be inside itself (a damaged project or file) is a gap with a report
entry in both directions; nothing recurses without a bound.

The AAF reader also accepts Premiere Pro's layout of a composition: all video tracks in ONE slot
(a nested scope with a segment per track, lowest first), and "top-level" tagged on nested
compositions too (the top-level ones are those no other composition uses).

## Graphics and adjustment layers

No format here has an equivalent of a FilmCraft graphic (title) clip or an adjustment layer, and
none of the exports brings one back. Each such clip is named in the export `Report` as a warning
(`graphic clip "Title" at frame 48 …`, one line per clip), with what was written instead:

| Format | Written as | On import into FilmCraft |
|---|---|---|
| FCP7 XML | a clip item without media (its layers as FilmCraft filter ids) | not read back; the import report names the skipped clip item |
| FCPXML | a clip without media | not read back |
| OTIO | a clip with a `MissingReference` (effects in the `filmcraft` metadata) | offline media |
| EDL | an ordinary event with the reel the reel mode gives | an ordinary clip |
| AAF | a gap | nothing |

OMF carries no video at all.

## Tests

- `tests/aaf_omf.rs`: AAF round trips at 25, 29.97 DF, 23.976 and 59.94 DF (clips, gaps,
  two-sided and one-sided transitions, links, markers, start timecodes, constant and keyframed
  gain, media markers and timecode); 512-byte sectors; breakout to mono; embedded trimmed audio
  (sample data and source offsets); separate consolidated files with a video mixdown; OMF round
  trips with embedded audio, separate AIFF files and breakout; empty sequences; truncation and
  random corruption never panic. Nested sequences: AAF round trips as compositions (nests in
  nests, a nest used twice, another frame and sample rate, breakout to mono), OMF with the
  rendered sound, the audio needs of nests, a sequence inside itself.
- `tests/uncarried_clips.rs`: every format's export report names each graphic clip and adjustment
  layer once, as a warning; a sequence without them reports nothing; what a re-import gives back
  matches the report; the FCP7 XML import names a clip item whose file is not defined.
- `src/aaf/tests.rs`: written files checked against the required properties of every class
  written (Header, Identification, Mobs, slots, components, descriptors), weak references
  resolving into the dictionary, source clips resolving to mobs and slots, the Edit Protocol
  transition rules and sequence length arithmetic; every stored form round-trips through the
  container; a file rewritten into Premiere Pro's layout (video tracks as layers of one slot,
  nested compositions tagged top-level) reads back as the same edit.
- `src/omf/tests.rs`: the Bento label and TOC, the required properties of every OMF class
  written, reference resolution, embedded WAVE data, sequence lengths in samples.
