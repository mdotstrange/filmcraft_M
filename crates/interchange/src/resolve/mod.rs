//! DaVinci Resolve project import: `.drp` exports (Project Manager ▸ Export Project).
//!
//! Clean-room: everything here was read off `.drp` files that DaVinci Resolve 21 exports from
//! projects we authored ourselves; no Resolve binary, SDK or database driver was consulted.
//!
//! A `.drp` is a ZIP archive ([`zipfile`]) of XML documents, one per database object tree:
//! `project.xml` (the project record), `MediaPool/Master/…/MpFolder.xml` (one per Media Pool
//! folder, nested like the folders), `SeqContainer/<uuid>.xml` (the tracks and items of one
//! timeline or compound clip) and `Gallery.xml` (stills; ignored). Binary fields are hex text.

pub mod blob;
mod build;
pub mod model;
pub mod xml;
pub mod zipfile;

use crate::{Error, Format, ImportOptions, Imported, Report, Result};

/// Most documents one export may hold.
const MAX_ENTRIES: usize = 1 << 16;

/// Whether `bytes` are a DaVinci Resolve project export: a ZIP archive whose central directory
/// lists `project.xml` at the root.
pub fn sniff(bytes: &[u8]) -> bool {
    zipfile::is_zip(bytes) && zipfile::Archive::parse(bytes).is_ok_and(|a| a.find("project.xml").is_some())
}

/// Import a `.drp`: every Media Pool folder and clip, every timeline and compound clip.
pub(crate) fn import(bytes: &[u8], opts: &ImportOptions, report: &mut Report) -> Result<Imported> {
    let err = |m: String| Error::parse(Format::ResolveProject, m);
    let archive = zipfile::Archive::parse(bytes).map_err(|e| err(e.to_string()))?;
    if archive.entries.len() > MAX_ENTRIES {
        return Err(err("too many documents in the export".into()));
    }
    let read = |name: &str| -> Result<String> {
        let e = archive.find(name).ok_or_else(|| err(format!("{name} is missing")))?;
        let data = archive.read(bytes, e).map_err(|x| err(format!("{name}: {x}")))?;
        Ok(xml::sanitise_tags(&String::from_utf8_lossy(&data)))
    };
    let project_text = read("project.xml")?;
    let project_doc = xml::parse(&project_text).map_err(|e| err(format!("project.xml: {e}")))?;
    let mut drp = model::Drp { project: model::parse_project(&project_doc).map_err(err)?, ..Default::default() };
    // Folders in path order (the archive writes them so; the `NNN_` prefixes keep Resolve's order).
    let mut folders: Vec<&str> =
        archive.entries.iter().map(|e| e.name.as_str()).filter(|n| n.starts_with("MediaPool/") && n.ends_with("/MpFolder.xml")).collect();
    folders.sort_unstable();
    for name in folders {
        match read(name).and_then(|t| xml::parse(&t).map_err(|e| err(format!("{name}: {e}"))).and_then(|d| model::parse_folder(&d).map_err(err))) {
            Ok(f) => drp.folders.push(f),
            Err(e) => report.warn(format!("{e}; folder skipped")),
        }
    }
    let containers: Vec<&str> = archive.entries.iter().map(|e| e.name.as_str()).filter(|n| n.starts_with("SeqContainer/") && n.ends_with(".xml")).collect();
    for name in containers {
        match read(name).and_then(|t| xml::parse(&t).map_err(|e| err(format!("{name}: {e}"))).and_then(|d| model::parse_container(&d).map_err(err))) {
            Ok(c) => {
                // Known by the container's own id and by the file's name (they agree in exports).
                if let Some(stem) = name.strip_prefix("SeqContainer/").and_then(|s| s.strip_suffix(".xml"))
                    && stem != c.id
                {
                    drp.containers.insert(stem.to_string(), c.clone());
                }
                drp.containers.insert(c.id.clone(), c);
            }
            Err(e) => report.warn(format!("{e}; timeline tracks skipped")),
        }
    }
    build::build(&drp, opts, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_needs_a_zip_with_project_xml() {
        assert!(sniff(&zipfile::write_stored(&[("project.xml", b"<SM_Project/>")])));
        assert!(!sniff(&zipfile::write_stored(&[("other.xml", b"<SM_Project/>")])));
        assert!(!sniff(b"<xmeml/>"));
        assert!(!sniff(b""));
        assert!(!sniff(b"PK\x03\x04garbage"));
    }
}
