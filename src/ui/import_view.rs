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
}

pub enum ImportOutcome {
    Open,
    Cancel,
    Import(Vec<Profile>),
}

impl ImportDialog {
    pub fn new(store: &ProfileStore) -> Self {
        let mut dialog = Self { rows: Vec::new(), notice: None };
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
            let exists = c.exists_in(store);
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
        let mut outcome = ImportOutcome::Open;
        let modal = egui::Modal::new(egui::Id::new("import_dialog")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Import hosts");
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
            if ui.small_button("Select all").clicked() {
                self.rows.iter_mut().for_each(|r| r.selected = true);
            }
            if ui.small_button("Select none").clicked() {
                self.rows.iter_mut().for_each(|r| r.selected = false);
            }
        });
        egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            for source in [Source::Bitvise, Source::OpenSsh] {
                if !self.rows.iter().any(|r| r.candidate.source == source) {
                    continue;
                }
                ui.add_space(4.0);
                ui.label(RichText::new(source.label().to_uppercase()).size(10.5).strong().color(theme::text_faint()));
                for row in self.rows.iter_mut().filter(|r| r.candidate.source == source) {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut row.selected, "");
                        let p = &row.candidate.profile;
                        ui.label(RichText::new(p.display_name()).strong());
                        ui.label(RichText::new(p.address()).size(12.0).color(theme::text_dim()));
                        if row.exists {
                            ui.label(RichText::new("already saved").size(11.0).color(theme::warn()));
                        }
                    })
                    .response
                    .on_hover_text(origin_label(&row.candidate.origin));
                }
            }
        });
    }
}

fn origin_label(p: &Path) -> String {
    format!("From {}", p.display())
}
