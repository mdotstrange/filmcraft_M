//! File ▸ Import From ▸ DaVinci Resolve Project…: listed in the menu, opens a `.drp` picker without
//! stopping the frame loop, imports the export as a new project when the picker answers, and
//! asks Save / Don't Save / Cancel first when the open project has unsaved changes. File ▸ Import
//! accepts `.drp` too.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_interchange::resolve::zipfile::write_stored;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use filmcraft_ui_egui::native_dialogs::FileDialog;
use serde_json::{Value, json};

type Asked = Arc<Mutex<Vec<(FileDialog, Sender<Vec<String>>)>>>;

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    asked: Asked,
}

impl Driver {
    fn new() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let mut app = FilmcraftApp::new(session).with_control(rx);
        let asked: Asked = Arc::default();
        let a = asked.clone();
        app.hooks.run_dialog = Some(Box::new(move |dialog: FileDialog| -> Receiver<Vec<String>> {
            let (tx, rx) = channel();
            a.lock().unwrap().push((dialog, tx));
            rx
        }));
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, asked };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).unwrap();
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                return v;
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let r = self.call(method, params.clone());
        assert_eq!(r["ok"], true, "{method} {params}: {r}");
        r["result"].clone()
    }

    fn click(&mut self, id: &str) {
        self.ok("ui.click", json!({"id": id}));
        self.frames(3);
    }

    fn app(&mut self) -> &mut FilmcraftApp {
        self.harness.state_mut()
    }

    fn asked(&mut self) -> (FileDialog, Sender<Vec<String>>) {
        let mut a = self.asked.lock().unwrap();
        assert_eq!(a.len(), 1, "one dialog asked for: {:?}", a.iter().map(|x| &x.0).collect::<Vec<_>>());
        a.pop().unwrap()
    }
}

fn f64_pair(a: f64, b: f64) -> String {
    let mut v = a.to_le_bytes().to_vec();
    v.extend(b.to_le_bytes());
    v.iter().map(|x| format!("{x:02x}")).collect()
}

/// A tiny export: one timeline "UI TL" at 24 fps (no track data, so it imports empty).
fn drp_file(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("resolve-import-ui");
    std::fs::create_dir_all(&dir).unwrap();
    let head = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
    let project = format!(
        "{head}<SM_Project DbId=\"p\"><ProjectName>UI Resolve Project</ProjectName><TimelineHandleVec><Element>tl</Element></TimelineHandleVec><CurrentTimelineIndex>0</CurrentTimelineIndex></SM_Project>"
    );
    let folder = format!(
        "{head}<Sm2MpFolder DbId=\"f\"><Name>Master</Name><MpFolder/><MediaVec><Element><Sm2MpTimelineClip DbId=\"m\"><Name>UI TL</Name>\
         <TimelineSharedHandle><Sm2Timeline DbId=\"tl\"><Name>UI TL</Name><Sequence><Sm2Sequence DbId=\"s\"><FrameRate>{}</FrameRate><MediaExtents>{}</MediaExtents>\
         <Resolution>00000000000007800000000000000438</Resolution></Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element></MediaVec></Sm2MpFolder>",
        f64_pair(24.0, 0.0),
        f64_pair(3600.0, 5.0)
    );
    let path = dir.join(format!("{tag}.drp"));
    std::fs::write(&path, write_stored(&[("project.xml", project.as_bytes()), ("MediaPool/Master/MpFolder.xml", folder.as_bytes())])).unwrap();
    path
}

#[test]
fn the_command_is_listed_under_file_import_from_and_the_import_filter_takes_drp() {
    let mut d = Driver::new();
    let menu = d.ok("ui.menu.list", json!({}));
    let entry = menu.as_array().unwrap().iter().find(|i| i["id"] == "file.importResolveProject").expect("listed in the menus");
    assert_eq!(entry["path"], json!(["File", "Import From"]));
    assert_eq!(d.ok("ui.menu.invoke", json!({"id": "file.import"})), json!({"dialog": "open"}));
    let (dialog, _answer) = d.asked();
    assert!(matches!(&dialog, FileDialog::OpenFiles { exts } if exts.iter().any(|e| e == "drp")), "{dialog:?}");
}

#[test]
fn picking_an_export_imports_it_as_a_new_project() {
    let mut d = Driver::new();
    // the demo project just opened is clean, so no save prompt stands in the way
    assert!(!d.app().session.is_dirty());
    assert_eq!(d.ok("ui.menu.invoke", json!({"id": "file.importResolveProject"})), json!({"dialog": "open"}));
    assert!(d.app().native_dialog_open());
    let (dialog, answer) = d.asked();
    assert!(matches!(&dialog, FileDialog::OpenFile { exts, .. } if exts == &["drp".to_string()]), "{dialog:?}");
    // the frame loop keeps running while the picker is open
    d.frames(10);
    answer.send(vec![drp_file("clean").to_string_lossy().into_owned()]).unwrap();
    d.frames(3);
    assert!(!d.app().native_dialog_open());
    let app = d.app();
    assert_eq!(app.session.project.name, "UI Resolve Project");
    assert!(app.session.project.items.values().any(|i| i.name == "UI TL" && i.as_sequence().is_some()));
    assert!(app.ui.status.contains("Imported") && app.ui.status.contains("UI Resolve Project"), "{}", app.ui.status);
}

#[test]
fn unsaved_changes_are_asked_about_before_the_export_replaces_the_project() {
    let mut d = Driver::new();
    d.app().session.execute("file.newSequence", json!({"name": "Unsaved work"})).unwrap();
    assert!(d.app().session.is_dirty());
    d.ok("ui.menu.invoke", json!({"id": "file.importResolveProject"}));
    let (_, answer) = d.asked();
    answer.send(vec![drp_file("dirty").to_string_lossy().into_owned()]).unwrap();
    d.frames(3);
    // the Save / Don't Save / Cancel prompt, not the import
    assert_eq!(d.app().session.project.name, "FilmCraft Demo", "nothing imported yet");
    let ids = d.ok("ui.elements", json!({}));
    let ids: Vec<&str> = ids.as_array().unwrap().iter().filter_map(|e| e["id"].as_str()).collect();
    assert!(ids.contains(&"importResolveProject.dontSave") && ids.contains(&"importResolveProject.cancel"), "{ids:?}");
    // Cancel keeps everything
    d.click("importResolveProject.cancel");
    assert!(d.app().session.project.items.values().any(|i| i.name == "Unsaved work"));
    assert_eq!(d.app().session.project.name, "FilmCraft Demo");
    // asked again, Don't Save imports
    d.ok("ui.menu.invoke", json!({"id": "file.importResolveProject"}));
    let (_, answer) = d.asked();
    answer.send(vec![drp_file("dirty").to_string_lossy().into_owned()]).unwrap();
    d.frames(3);
    d.click("importResolveProject.dontSave");
    let app = d.app();
    assert_eq!(app.session.project.name, "UI Resolve Project");
    assert!(!app.session.project.items.values().any(|i| i.name == "Unsaved work"));
    assert!(app.ui.clip_dialog.is_none());
}
