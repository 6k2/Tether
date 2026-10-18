//! Opening remote files from the SFTP view: the built-in editor (via the app command bus)
//! or the PC's default app with change watching, plus the "watching" chip in the header.
//! A child module of `sftp_view` so it can use the view's private state.

use std::sync::atomic::Ordering;
use std::time::Instant;

use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Sense, Stroke, Ui, Vec2};

use super::SftpView;
use super::dialogs::Dialog;
use crate::commands::{self, AppCommand, EditorDoc};
use crate::remote_edit::{self, MAX_EDITOR_BYTES, Refusal, WatchContext, WatchEntry, WatchStatus};
use crate::settings::{self, OpenWith};
use crate::ssh::sftp;
use crate::theme;
use crate::ui::sftp_data::set;

impl SftpView {
    /// Double-click / Enter on a remote file.
    pub(super) fn open_remote_file(&mut self, name: String) {
        match settings::get().open_with_for(&name) {
            OpenWith::Editor => self.open_in_editor(name),
            OpenWith::System => self.open_with_system(name),
        }
    }

    /// Downloads the file and asks the app for an editor tab; binary, non-UTF-8 and huge
    /// files are refused with an offer to use the system app instead.
    pub(super) fn open_in_editor(&mut self, name: String) {
        let Some(sftp) = self.sftp() else { return };
        let path = sftp::join(&self.remote_cwd(), &name);
        let (profile, refused, opening, state, ctx) =
            (self.profile.clone(), self.open_refused.clone(), self.opening.clone(), self.remote.clone(), self.ctx.clone());
        opening.fetch_add(1, Ordering::Relaxed);
        self.rt.spawn(async move {
            let refusal = match remote_edit::read_capped(&sftp, &path, MAX_EDITOR_BYTES).await {
                Ok(bytes) => match remote_edit::decode(&bytes) {
                    Ok(_) => {
                        let pipeline_kb = profile.pipeline_kb;
                        commands::push(AppCommand::OpenEditor(Box::new(EditorDoc { profile, remote_path: path, bytes, sftp, pipeline_kb })));
                        None
                    }
                    Err(r) => Some(r),
                },
                Err(e) => match e.downcast_ref::<Refusal>() {
                    Some(r) => Some(r.clone()),
                    None => {
                        set(&state, |s| s.error = Some(format!("{e:#}")));
                        None
                    }
                },
            };
            if let Some(r) = refusal {
                set(&refused, |slot| *slot = Some((name, r.to_string())));
            }
            opening.fetch_sub(1, Ordering::Relaxed);
            ctx.request_repaint();
        });
    }

    /// Bitvise-style: download to a temp folder, open with the default app, upload on save.
    pub(super) fn open_with_system(&mut self, name: String) {
        let Some(sftp) = self.sftp() else { return };
        let path = sftp::join(&self.remote_cwd(), &name);
        let wc = WatchContext { sftp, pipeline_kb: self.profile.pipeline_kb, rt: self.rt.clone(), ctx: self.ctx.clone() };
        self.watcher.open(path, wc);
    }

    pub(super) fn always_open_with(&mut self, ext: String, with: OpenWith) {
        if let Err(e) = settings::update(|s| {
            s.open_with.insert(ext, with);
        }) {
            set(&self.remote, |s| s.error = Some(format!("Saving settings failed: {e:#}")));
        }
    }

    /// Per-frame bookkeeping: refusal dialogs from background opens, and a listing
    /// refresh after the watcher uploaded into the current folder.
    pub(super) fn poll_open(&mut self) {
        if self.dialog.is_none()
            && let Some((name, reason)) = self.open_refused.lock().ok().and_then(|mut s| s.take())
        {
            self.dialog = Some(Dialog::OpenRefused { name, reason });
        }
        let dirs = self.watcher.take_uploaded_dirs();
        if !dirs.is_empty() && dirs.contains(&self.remote_cwd()) {
            self.remote_go(super::NavOp::Reload);
        }
    }

    pub(super) fn is_opening(&self) -> bool {
        self.opening.load(Ordering::Relaxed) > 0
    }

    /// "Watching 2 files · last upload 12s ago" chip; click for the list and stop buttons.
    pub(super) fn watch_chip(&self, ui: &mut Ui) {
        let entries = self.watcher.entries();
        if entries.is_empty() {
            return;
        }
        let busy = entries.iter().any(|e| matches!(e.status, WatchStatus::Downloading | WatchStatus::Uploading));
        let failed = entries.iter().any(|e| matches!(e.status, WatchStatus::Failed(_)) || e.last_error.is_some());
        let last = entries.iter().filter_map(|e| e.last_upload).max();
        let n = entries.len();
        let mut text = format!("Watching {n} file{}", if n == 1 { "" } else { "s" });
        if let Some(t) = last {
            text.push_str(&format!(" · last upload {}", ago(t)));
            // Keep the relative time fresh.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        let color = if failed {
            theme::danger()
        } else if busy {
            theme::blue()
        } else {
            theme::accent()
        };
        let resp = chip(ui, &text, color, busy);
        let resp = resp.on_hover_text("Files opened with a PC app. Every save there is uploaded to the server.\nClick to manage.");
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(300.0);
            ui.label(RichText::new("Opened with system apps").size(11.0).color(theme::text_faint()));
            for e in &entries {
                self.watch_row(ui, e);
            }
        });
    }

    fn watch_row(&self, ui: &mut Ui, e: &WatchEntry) {
        ui.horizontal(|ui| {
            let (status, color) = match &e.status {
                WatchStatus::Downloading => ("downloading…".to_string(), theme::blue()),
                WatchStatus::Uploading => ("uploading…".to_string(), theme::blue()),
                WatchStatus::Failed(_) => ("failed".to_string(), theme::danger()),
                WatchStatus::Watching if e.last_error.is_some() => ("upload failed".to_string(), theme::danger()),
                WatchStatus::Watching => match e.last_upload {
                    Some(t) => (format!("uploaded {}", ago(t)), theme::accent()),
                    None => ("watching".to_string(), theme::text_dim()),
                },
            };
            let name = ui.label(RichText::new(e.file_name()).color(theme::text()));
            let tip = match (&e.status, &e.last_error) {
                (WatchStatus::Failed(err), _) | (_, Some(err)) => format!("{}\n{err}", e.remote_path),
                _ => e.remote_path.clone(),
            };
            name.on_hover_text(tip);
            ui.label(RichText::new(status).size(11.0).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if matches!(e.status, WatchStatus::Failed(_)) { "Dismiss" } else { "Stop" };
                if ui.small_button(label).on_hover_text("Stop watching and delete the local copy").clicked() {
                    self.watcher.dismiss(e.id);
                }
            });
        });
    }
}

fn ago(t: Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..5 => "just now".into(),
        5..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

fn chip(ui: &mut Ui, text: &str, color: Color32, pulse: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), theme::text_dim());
    let size = Vec2::new(galley.size().x + 28.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    let radius = CornerRadius::same(10);
    p.rect_filled(rect, radius, theme::surface_2().lerp_to_gamma(theme::surface_3(), hover));
    p.rect_stroke(rect, radius, Stroke::new(1.0, color.gamma_multiply(0.35 + 0.3 * hover)), egui::StrokeKind::Inside);
    let alpha = if pulse {
        ui.ctx().request_repaint();
        0.4 + 0.6 * ((ui.input(|i| i.time) * 5.0).sin() * 0.5 + 0.5) as f32
    } else {
        1.0
    };
    p.circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.5, color.gamma_multiply(alpha));
    p.galley(egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0), galley, theme::text_dim());
    resp
}
