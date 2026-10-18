//! Interchange documents (CMX 3600 EDL, FCP7 XML, FCPXML, OTIO, AAF, OMF, DaVinci Resolve `.drp`)
//! ↔ the session's project.

use std::collections::HashSet;

use serde_json::{Value, json};

use filmcraft_interchange::{ExportOptions, Format, ImportOptions};
use filmcraft_project::{BinId, ItemId, ItemKind, MediaRef};

use crate::commands::bad;
use crate::{EngineError, Result, Session};

/// The interchange format of a file, if it is one (by content, with the extension as a hint).
pub fn detect(path: &str, bytes: &[u8]) -> Option<Format> {
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if !matches!(ext.as_deref(), Some("edl" | "xml" | "fcpxml" | "otio" | "aaf" | "omf" | "omfi" | "drp")) {
        return None;
    }
    filmcraft_interchange::detect(bytes, ext.as_deref())
}

fn report_json(r: &filmcraft_interchange::Report) -> Vec<String> {
    r.entries.iter().map(|e| if e.count > 1 { format!("{} (×{})", e.message, e.count) } else { e.message.clone() }).collect()
}

fn is_file_media(item: &filmcraft_project::ProjectItem) -> bool {
    matches!(&item.kind, ItemKind::Media(m) if matches!(m.media, MediaRef::File { .. }))
}

/// Import a document into the project root: see [`import_into`].
pub fn import(s: &mut Session, path: &str, bytes: &[u8], format: Format) -> Result<Value> {
    import_into(s, path, bytes, format, None)
}

/// Import a document: merge its bins, media and sequences into `bin` of the project (the root
/// when `None`), then link every media file that exists on disk. Two undo steps: the merge and
/// one "Link Media" step for all the files found (a Resolve project can list thousands). Returns
/// the new sequences.
pub fn import_into(s: &mut Session, path: &str, bytes: &[u8], format: Format, bin: Option<BinId>) -> Result<Value> {
    let p = std::path::Path::new(path);
    let opts = ImportOptions {
        base_dir: p.parent().map(|d| d.to_string_lossy().to_string()),
        name: p.file_stem().map(|n| n.to_string_lossy().to_string()),
        ..Default::default()
    };
    let (fragment, report) = match format {
        // AAF / OMF may embed audio: write it next to the document (the media items point there)
        Format::Aaf | Format::Omf => {
            let r = if format == Format::Aaf { filmcraft_interchange::aaf::import(bytes, &opts) } else { filmcraft_interchange::omf::import(bytes, &opts) };
            let (fragment, extracted, report) = r.map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
            for m in extracted {
                if let Some(dir) = std::path::Path::new(&m.path).parent().filter(|_| !s.services.export_in_memory()) {
                    let _ = std::fs::create_dir_all(dir);
                }
                s.services.write_file(&m.path, &m.wav).map_err(|e| EngineError::Other(format!("{}: {e}", m.path)))?;
            }
            (fragment, report)
        }
        _ => filmcraft_interchange::import_with(bytes, format, &opts).map_err(|e| EngineError::Other(format!("{path}: {e}")))?,
    };
    let before: HashSet<ItemId> = s.project.items.keys().copied().collect();
    let name = opts.name.clone().unwrap_or_else(|| format.name().to_string());
    let seqs = s.edit(&format!("Import {name}"), |proj, _| {
        let seqs = filmcraft_interchange::merge_into(proj, fragment, bin);
        // A document that sets only some Motion parameters leaves the others at their "auto"
        // (NaN) defaults. Resolve them as placing a clip does, for every format. A media file's
        // picture size is only what the document says so far (an EDL says nothing: the importer
        // assumes the sequence size), so its clips' anchors wait for Link Media below.
        proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| !is_file_media(source));
        Ok(seqs)
    })?;
    // Link media: probe each new file-backed item that exists, through the host's reader as a
    // direct import does (only the container index is read now, not the whole file: #461).
    let new_media: Vec<(ItemId, String)> = s
        .project
        .items
        .values()
        .filter(|i| !before.contains(&i.id))
        .filter_map(|i| match &i.kind {
            ItemKind::Media(m) => match &m.media {
                MediaRef::File { path } => Some((i.id, path.clone())),
                _ => None,
            },
            _ => None,
        })
        .collect();
    // The document's media paths are opened as stored paths of the document.
    s.media.add_stored_paths(new_media.iter().map(|(_, p)| p.clone()), Some(path));
    let mut opened = Vec::new();
    let mut offline = Vec::new();
    for (id, mpath) in new_media {
        let file = match s.media.follow(&mpath) {
            Ok(file) => file,
            Err(why) => {
                log::warn!("{mpath} is not opened: {why}");
                offline.push(mpath);
                continue;
            }
        };
        match s.media.open_file(&file, &*s.services).ok() {
            Some(src) => {
                let identity = crate::relink::identity_of(&*s.services, &file).ok();
                opened.push((id, mpath, src.info().clone(), identity, src));
            }
            None => offline.push(mpath),
        }
    }
    let linked = opened.len();
    if !opened.is_empty() {
        let updates: Vec<_> = opened.iter().map(|(id, _, info, identity, _)| (*id, info.clone(), *identity)).collect();
        let rebase_edl = format == Format::Edl;
        s.edit("Link Media", |proj, _| {
            let mut linked: HashSet<ItemId> = HashSet::new();
            for (id, info, identity) in updates {
                let rate = info.video.as_ref().map(|v| v.frame_rate);
                let rebase = rebase_edl.then_some(info.start_timecode).flatten();
                let Some(ItemKind::Media(m)) = proj.items.get_mut(&id).map(|i| &mut i.kind) else { continue };
                m.info = info;
                m.offline = false;
                m.identity = identity;
                if let (Some(tc), Some(rate)) = (rebase, rate) {
                    filmcraft_interchange::rebase_source_timecode(proj, id, tc, rate);
                }
                linked.insert(id);
            }
            // the picture sizes are now the files' own: centre the anchors of their clips in them
            proj.resolve_placed_auto_points(|seq| !before.contains(&seq), |source| linked.contains(&source.id));
            Ok(())
        })?;
        for (id, mpath, _, _, src) in opened {
            s.media.insert_file(id, &mpath, src);
        }
    }
    if let Some(&first) = seqs.first() {
        s.state.active_sequence = Some(first);
        if !s.state.open_sequences.contains(&first) {
            s.state.open_sequences.push(first);
        }
    }
    Ok(json!({
        "format": format.name(),
        "sequences": seqs.iter().map(|i| i.0).collect::<Vec<_>>(),
        "linkedMedia": linked,
        "offlineMedia": offline,
        "report": report_json(&report),
    }))
}

/// The project name a Resolve export carries, without importing it.
fn resolve_project_name(bytes: &[u8]) -> Option<String> {
    use filmcraft_interchange::resolve::{model, xml, zipfile};
    let archive = zipfile::Archive::parse(bytes).ok()?;
    let entry = archive.find("project.xml")?;
    let data = archive.read(bytes, entry).ok()?;
    let text = xml::sanitise_tags(&String::from_utf8_lossy(&data));
    let doc = xml::parse(&text).ok()?;
    let name = model::parse_project(&doc).ok()?.name;
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

/// `file.importResolveProject {path, mode: "newProject"|"intoBin" = "newProject", bin?}`: a
/// DaVinci Resolve `.drp` export as a new project named after the Resolve project (the default),
/// or as a bin of that name in the open project (`bin` = where that bin goes). One undo step.
pub fn import_resolve(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "file.importResolveProject";
    let path =
        p.get("path").and_then(Value::as_str).map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "need `path` (a .drp export)"))?.to_string();
    let into_bin = match p.get("mode").and_then(Value::as_str).unwrap_or("newProject") {
        "newProject" => false,
        "intoBin" => true,
        other => return Err(bad(CMD, format!("unknown mode {other:?}: use newProject or intoBin"))),
    };
    let parent = p.get("bin").and_then(Value::as_u64).map(BinId);
    let bytes = s.services.read_file(&path).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if detect(&path, &bytes) != Some(Format::ResolveProject) {
        return Err(bad(CMD, format!("{path} is not a DaVinci Resolve project export (.drp)")));
    }
    let name = resolve_project_name(&bytes)
        .or_else(|| std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "Resolve Project".to_string());
    let mut r = if into_bin {
        s.edit_group("Import DaVinci Resolve Project", |s| {
            let bin = s.edit("New Bin", |pr, _| Ok(pr.add_bin(&name, parent)))?;
            import_into(s, &path, &bytes, Format::ResolveProject, Some(bin))
        })?
    } else {
        // Not undoable, like `file.import {replace: true}`: the previous project is closed
        // (its unsaved changes stay in the recovery journal) and the import fills the new one.
        s.execute("file.newProject", json!({ "name": name }))?;
        s.edit_group("Import DaVinci Resolve Project", |s| import_into(s, &path, &bytes, Format::ResolveProject, None))?
    };
    // the "Media missing" badge and the Link Media prompt follow the new items
    crate::relink::refresh(s);
    r["project"] = json!(s.project.name);
    r["mode"] = json!(if into_bin { "intoBin" } else { "newProject" });
    Ok(r)
}

/// `file.exportInterchange {format: "edl"|"xml"|"fcpxml"|"otio"|"aaf"|"omf" = "xml", path, sequence?}`
pub fn export(s: &mut Session, p: &Value) -> Result<Value> {
    // FCP7 XML only when no format is asked for: a format we don't know is an error, not XML.
    // The names are the documented ones (each format's extension, in either case) and nothing
    // else, so the error can list exactly what is accepted.
    let format = match p.get("format") {
        None | Some(Value::Null) => Format::Fcp7Xml,
        Some(v) => v.as_str().and_then(|name| Format::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(name))).ok_or_else(|| {
            let names: Vec<&str> = Format::ALL.iter().map(|f| f.extension()).collect();
            crate::commands::bad("file.exportInterchange", format!("unknown format {v}: use one of {}", names.join(", ")))
        })?,
    };
    let path = p.get("path").and_then(Value::as_str).ok_or_else(|| EngineError::Other("need `path`".into()))?.to_string();
    let seq = p.get("sequence").and_then(Value::as_u64).map(ItemId).or(s.state.active_sequence).ok_or(EngineError::NoSequence)?;
    let opts = ExportOptions {
        relative_to: std::path::Path::new(&path).parent().map(|d| d.to_string_lossy().to_string()),
        name: s.project.item(seq).map(|i| i.name.clone()),
        ..Default::default()
    };
    let (bytes, report) = filmcraft_interchange::export(&s.project, seq, format, &opts).map_err(|e| EngineError::Other(e.to_string()))?;
    s.services.write_file(&path, &bytes).map_err(|e| EngineError::Other(e.to_string()))?;
    Ok(json!({"path": path, "bytes": bytes.len(), "report": report_json(&report)}))
}
