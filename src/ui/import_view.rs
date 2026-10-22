//! "Import hosts" modal: auto-detected Bitvise profiles and SSH config hosts, plus
//! file pickers for profiles stored elsewhere.

use std::path::Path;

use eframe::egui::{self, RichText, Ui};

use crate::import::{Candidate, Source, bitvise, openssh};
use crate::profile::{Profile, ProfileStore};
use crate::theme;

struct Row {
    candidate: Candidate,
    selected: bool,
    exists: bool,
}

pub struct ImportDialog {
    rows: Vec<Row>,
    notice: Option<String>,
    export: bool,
    acknowledged: bool,
    scan: Option<std::sync::mpsc::Receiver<crate::import::discovery::Found>>,
    searched: Vec<std::path::PathBuf>,
}

pub enum ImportOutcome {
    Open,
    Cancel,
    Import(Vec<Profile>),
}

impl ImportDialog {
    pub fn new(store: &ProfileStore) -> Self {
        let mut dialog = Self { rows: Vec::new(), notice: None, export: false, acknowledged: false, scan: None, searched: Vec::new() };
        dialog.add(bitvise::discover(), store);
        if let Some(path) = openssh::default_path() {
            dialog.add(openssh::load(&path), store);
        }
        dialog
    }

    fn add(&mut self, candidates: Vec<Candidate>, store: &ProfileStore) -> usize {
        let mut added = 0;
        for c in candidates {
            let duplicate = self.rows.iter().any(|r| r.candidate.origin == c.origin && r.candidate.profile.name == c.profile.name);
            if duplicate {
                continue;
            }
            let exists = c.exists_in(store)
                || self.rows.iter().any(|r| {
                    let p = &r.candidate.profile;
                    p.host.eq_ignore_ascii_case(&c.profile.host) && p.port == c.profile.port && p.user == c.profile.user
                });
            self.rows.push(Row { candidate: c, selected: !exists, exists });
            added += 1;
        }
        added
    }

    fn pick_bitvise(&mut self, store: &ProfileStore) {
        let start = bitvise::search_dirs().into_iter().find(|d| d.is_dir()).unwrap_or_default();
        let Some(files) = rfd::FileDialog::new()
            .set_title("Choose Bitvise profiles")
            .set_directory(start)
            .add_filter("Bitvise profile", &["tlp"])
            .add_filter("All files", &["*"])
            .pick_files()
        else {
            return;
        };
        let found: Vec<Candidate> = files.iter().filter_map(|f| bitvise::load(f)).collect();
        let skipped = files.len() - found.len();
        let added = self.add(found, store);
        self.notice = Some(match skipped {
            0 => format!("{added} profile(s) added"),
            n => format!("{added} profile(s) added · {n} file(s) were not Bitvise profiles"),
        });
    }

    fn pick_ssh_config(&mut self, store: &ProfileStore) {
        let Some(file) = rfd::FileDialog::new().set_title("Choose an SSH config file").pick_file() else { return };
        let added = self.add(openssh::load(&file), store);
        self.notice = Some(format!("{added} host(s) added from {}", file.display()));
    }

    pub fn show(&mut self, ctx: &egui::Context, store: &ProfileStore) -> ImportOutcome {
        if let Some(result) = self.scan.as_ref().map(|rx| rx.try_recv()) {
            match result {
                Ok(found) => {
                    self.searched = found.searched;
                    let n = self.add(found.profiles, store);
                    self.notice = Some(format!(
                        "Found {n} Bitvise profile(s){}",
                        if found.limited { " · Search limit reached; use Browse for other folders" } else { "" }
                    ));
                    self.scan = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.notice = Some("Search stopped. You can still browse for files manually.".into());
                    self.scan = None;
                }
                Err(_) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let mut outcome = ImportOutcome::Open;
        let modal = egui::Modal::new(egui::Id::new("import_dialog")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Import / export machines");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.export, false, "Import");
                ui.selectable_value(&mut self.export, true, "Export setup");
            });
            if self.export {
                self.export_ui(ui, store);
                if ui.button("Close").clicked() {
                    outcome = ImportOutcome::Cancel;
                }
                return;
            }
            ui.label(RichText::new("Connection details only — passwords are not imported.").size(12.0).color(theme::text_dim()));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("📁  Bitvise profiles…").clicked() {
                    self.pick_bitvise(store);
                }
                if ui.button("📄  SSH config…").clicked() {
                    self.pick_ssh_config(store);
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Import Tether setup…").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Tether setup", &["json"]).pick_file() {
                        self.notice = Some(match crate::import::setup::load(&path) {
                            Ok(candidates) => {
                                let n = self.add(candidates, store);
                                format!("{n} host(s) ready to import")
                            }
                            Err(e) => format!("Could not import: {e:#}"),
                        });
                    }
                }
                if ui.add_enabled(self.scan.is_none(), egui::Button::new("Search this PC")).clicked() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.scan = Some(rx);
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        let _ = tx.send(crate::import::discovery::scan(crate::import::discovery::roots()));
                        ctx.request_repaint();
                    });
                }
                if self.scan.is_some() {
                    ui.spinner();
                }
            });
            if !self.searched.is_empty() {
                ui.collapsing("Searched locations", |ui| {
                    for path in &self.searched {
                        ui.label(path.display().to_string());
                    }
                });
            }
            if let Some(n) = &self.notice {
                ui.label(RichText::new(n).size(12.0).color(theme::accent()));
            }
            ui.add_space(6.0);
            self.list(ui);
            ui.add_space(10.0);
            let count = self.rows.iter().filter(|r| r.selected).count();
            ui.horizontal(|ui| {
                let label = RichText::new(format!("Import {count}")).color(egui::Color32::WHITE);
                if ui.add_enabled(count > 0, egui::Button::new(label).fill(theme::accent().gamma_multiply(0.8))).clicked() {
                    let profiles = self.rows.iter().filter(|r| r.selected).map(|r| r.candidate.profile.clone()).collect();
                    outcome = ImportOutcome::Import(profiles);
                }
                if ui.button("Cancel").clicked() {
                    outcome = ImportOutcome::Cancel;
                }
            });
        });
        if modal.should_close() && matches!(outcome, ImportOutcome::Open) {
            outcome = ImportOutcome::Cancel;
        }
        outcome
    }

    fn export_ui(&mut self, ui: &mut Ui, store: &ProfileStore) {
        ui.add_space(8.0);
        ui.label(
            RichText::new("Anyone with this file can import your machine names, IP addresses / hostnames, ports, usernames and colors.").color(theme::warn()),
        );
        ui.label("SSH keys, key paths, passwords and passphrases are excluded. Imported machines require credentials to connect.");
        ui.add_space(8.0);
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            for p in &store.profiles {
                ui.label(format!("{} · {}", p.display_name(), p.address()));
            }
        });
        ui.checkbox(&mut self.acknowledged, "I understand the file contains connection details");
        if ui.add_enabled(self.acknowledged && !store.profiles.is_empty(), egui::Button::new(format!("Export {} machines…", store.profiles.len()))).clicked()
        {
            if let Some(path) = rfd::FileDialog::new().set_file_name("tether-setup.json").add_filter("Tether setup", &["json"]).save_file() {
                let result = crate::import::setup::encode(&store.profiles).and_then(|text| std::fs::write(&path, text).map_err(Into::into));
                self.notice = Some(match result {
                    Ok(()) => format!("Exported to {}", path.display()),
                    Err(e) => format!("Export failed: {e:#}"),
                });
            }
        }
        if let Some(n) = &self.notice {
            ui.label(n);
        }
    }

    fn list(&mut self, ui: &mut Ui) {
        if self.rows.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Nothing detected automatically").color(theme::text_dim()));
                ui.label(RichText::new("Choose Bitvise .tlp files or an SSH config above").size(11.5).color(theme::text_faint()));
            });
            ui.add_space(20.0);
            return;
        }
        ui.horizontal(|ui| {
            if ui.small_button("Select new").clicked() {
                self.rows.iter_mut().for_each(|r| r.selected = !r.exists);
            }
            if ui.small_button("Select none").clicked() {
                self.rows.iter_mut().for_each(|r| r.selected = false);
            }
        });
        egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            for source in [Source::Tether, Source::Bitvise, Source::OpenSsh] {
                if !self.rows.iter().any(|r| r.candidate.source == source) {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(RichText::new(source.label().to_uppercase()).size(10.5).strong().color(theme::text_faint()));
                for row in self.rows.iter_mut().filter(|r| r.candidate.source == source) {
                    ui.horizontal(|ui| {
                        ui.add_enabled(!row.exists, egui::Checkbox::new(&mut row.selected, ""));
                        let p = &row.candidate.profile;
                        ui.label(RichText::new(p.display_name()).strong());
                        ui.label(RichText::new(p.address()).size(12.0).color(theme::text_dim()));
                        if row.exists {
                            ui.label(RichText::new("already saved").size(11.0).color(theme::warn()));
                        }
                    })
                    .response
                    .on_hover_text(origin_label(&row.candidate.origin));
                    ui.label(RichText::new(row.candidate.origin.display().to_string()).size(10.5).color(theme::text_faint()));
                }
            }
        });
    }
}

fn origin_label(p: &Path) -> String {
    format!("From {}", p.display())
}
