//! Modal dialogs of the SFTP view (new folder, rename, delete, editor refusal). A child module of
//! `sftp_view` so it can drive the view's private state.

use std::path::PathBuf;

use eframe::egui::{self, Color32};

use super::SftpView;
use crate::ssh::sftp;
use crate::theme;
use crate::ui::file_list::Side;

pub(super) enum Dialog {
    NewFolder {
        side: Side,
        name: String,
    },
    Rename {
        side: Side,
        old: String,
        new: String,
    },
    Delete {
        side: Side,
        names: Vec<String>,
    },
    /// The editor refused a remote file; offer the system app instead.
    OpenRefused {
        name: String,
        reason: String,
    },
}

impl SftpView {
    pub(super) fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let mut confirmed = false;
        let mut dialog = dialog;
        let modal = egui::Modal::new(egui::Id::new("sftp_dialog")).show(ctx, |ui| {
            ui.set_width(320.0);
            match &mut dialog {
                Dialog::NewFolder { name, .. } => {
                    ui.heading("New folder");
                    let r = ui.add(egui::TextEdit::singleline(name).hint_text("folder name").desired_width(f32::INFINITY));
                    r.request_focus();
                    confirmed |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
                Dialog::Rename { new, .. } => {
                    ui.heading("Rename");
                    let r = ui.add(egui::TextEdit::singleline(new).desired_width(f32::INFINITY));
                    r.request_focus();
                    confirmed |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                }
                Dialog::Delete { names, .. } => {
                    ui.heading("Delete permanently?");
                    let preview = names.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
                    let more = names.len().saturating_sub(5);
                    ui.label(if more > 0 { format!("{preview} and {more} more") } else { preview });
                    ui.colored_label(theme::danger(), "Folders are deleted recursively. This cannot be undone.");
                }
                Dialog::OpenRefused { name, reason } => {
                    ui.heading(format!("Can't open {name} in the editor"));
                    ui.label(egui::RichText::new(reason.as_str()).color(theme::text_dim()));
                    ui.label("Open it with the default app on this PC instead? Saved changes are uploaded automatically.");
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let label = match dialog {
                    Dialog::Delete { .. } => "Delete",
                    Dialog::OpenRefused { .. } => "Open with system app",
                    _ => "OK",
                };
                let color = if matches!(dialog, Dialog::Delete { .. }) { theme::danger() } else { theme::accent() };
                confirmed |= ui.add(egui::Button::new(egui::RichText::new(label).color(Color32::WHITE)).fill(color.gamma_multiply(0.75))).clicked();
                if ui.button("Cancel").clicked() {
                    keep = false;
                }
            });
        });
        if modal.should_close() {
            keep = false;
        }
        if confirmed {
            self.run_dialog(dialog);
        } else if keep {
            self.dialog = Some(dialog);
        }
    }

    fn run_dialog(&mut self, dialog: Dialog) {
        let rcwd = self.remote_cwd();
        match dialog {
            Dialog::NewFolder { side: Side::Local, name } => {
                let p = self.local_cwd.join(name.trim());
                self.local_op(|| std::fs::create_dir(p));
            }
            Dialog::NewFolder { side: Side::Remote, name } => {
                let p = sftp::join(&rcwd, name.trim());
                self.remote_op(move |s| async move { s.mkdir(&p).await });
            }
            Dialog::Rename { side: Side::Local, old, new } => {
                let (a, b) = (self.local_cwd.join(old), self.local_cwd.join(new.trim()));
                // std::fs::rename replaces an existing target on Windows; never do that silently.
                self.local_op(|| {
                    if b.exists() {
                        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("{} already exists", b.display())));
                    }
                    std::fs::rename(a, b)
                });
            }
            Dialog::Rename { side: Side::Remote, old, new } => {
                let (a, b) = (sftp::join(&rcwd, &old), sftp::join(&rcwd, new.trim()));
                self.remote_op(move |s| async move { s.rename(&a, &b).await });
            }
            Dialog::Delete { side: Side::Local, names } => {
                let paths: Vec<PathBuf> = names.iter().map(|n| self.local_cwd.join(n)).collect();
                self.local_op(|| {
                    for p in paths {
                        if p.is_dir() { std::fs::remove_dir_all(p)? } else { std::fs::remove_file(p)? }
                    }
                    Ok(())
                });
                self.local_list.clear();
            }
            Dialog::Delete { side: Side::Remote, names } => {
                let rows = self.remote.lock().map(|s| s.rows.clone()).unwrap_or_default();
                let targets: Vec<(String, bool)> =
                    names.iter().map(|n| (sftp::join(&rcwd, n), rows.iter().any(|r| &r.name == n && r.is_dir && !r.is_link))).collect();
                self.remote_list.clear();
                self.remote_op(move |s| async move {
                    for (path, is_dir) in targets {
                        s.remove(&path, is_dir).await?;
                    }
                    Ok(())
                });
            }
            Dialog::OpenRefused { name, .. } => self.open_with_system(name),
        }
    }
}
