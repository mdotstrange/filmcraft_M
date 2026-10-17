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
pub mod model;
pub mod xml;
pub mod zipfile;

/// Whether `bytes` are a DaVinci Resolve project export: a ZIP archive whose central directory
/// lists `project.xml` at the root.
pub fn sniff(bytes: &[u8]) -> bool {
    zipfile::is_zip(bytes) && zipfile::Archive::parse(bytes).is_ok_and(|a| a.find("project.xml").is_some())
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
