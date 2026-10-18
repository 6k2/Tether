//! Key manager (a simpler Bitvise client key manager): list, generate, import, view
//! (fingerprint, randomart, public key), rename / re-comment, change the passphrase,
//! export, install on a host and delete. Everything that runs the key KDF or generates
//! a key (tens of ms to seconds) happens on a worker thread; the UI shows a spinner.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use russh::keys::PrivateKey;
use uuid::Uuid;

use super::loading;
use super::settings_widgets as w;
use crate::keystore::{self, KeyKind, KeyMeta, KeyStore, Rewrite};
use crate::profile::{AuthMethod, Profile, ProfileStore, secret};
use crate::theme;

const LIST_W: f32 = 270.0;
const CARD_H: f32 = 58.0;

pub enum KeyOutcome {
    Open,
    Close,
    /// A key was generated or imported while the host editor asked for one.
    Created(Uuid),
    /// Append the public key to `~/.ssh/authorized_keys` on this host.
    Install {
        key: Uuid,
        host: Uuid,
    },
}

#[derive(Clone)]
struct GenForm {
    name: String,
    kind: KeyKind,
    comment: String,
    pass: String,
    pass2: String,
    remember: bool,
}

impl Default for GenForm {
    fn default() -> Self {
        Self { name: String::new(), kind: KeyKind::Ed25519, comment: keystore::default_comment(), pass: String::new(), pass2: String::new(), remember: true }
    }
}

enum Panel {
    Details,
    Generate(GenForm),
}

/// Inline form under the detail actions.
enum Form {
    None,
    Edit { name: String, comment: String, pass: String },
    Passphrase { old: String, new: String, new2: String, remember: bool },
    Delete,
}

/// What a finished worker job hands back to the UI thread.
enum Done {
    /// A new key, already encrypted as the form asked.
    Generated { form: GenForm, key: Box<PrivateKey> },
    /// A re-encoded key file, plus what to do with the label and the keyring.
    Rewritten { rewrite: Rewrite, id: Uuid, name: Option<String>, remember: Remember, note: String },
}

/// Keyring change applied once a passphrase change succeeded.
enum Remember {
    Keep,
    Store(String),
    Forget,
}

type Job = Arc<Mutex<Option<anyhow::Result<Done>>>>;

pub struct KeyManager {
    selected: Option<Uuid>,
    panel: Panel,
    form: Form,
    /// Worker job in flight (generation or a passphrase / comment rewrite).
    pending: Option<Job>,
    message: Option<(bool, String)>,
    /// Hand the next created key back to the host editor.
    for_editor: bool,
    install_host: Option<Uuid>,
    opened_at: Option<f64>,
}

impl KeyManager {
    pub fn new(keys: &KeyStore) -> Self {
        Self {
            selected: keys.keys.first().map(|k| k.id),
            panel: Panel::Details,
            form: Form::None,
            pending: None,
            message: None,
            for_editor: false,
            install_host: None,
            opened_at: None,
        }
    }

    /// Opens straight on the generate form; the new key is reported as `Created`.
    pub fn generate_for_editor(keys: &KeyStore) -> Self {
        Self { panel: Panel::Generate(GenForm::default()), for_editor: true, ..Self::new(keys) }
    }

    /// Opened from the host editor's "Manage": imports are handed back to it too.
    pub fn for_editor(mut self) -> Self {
        self.for_editor = true;
        self
    }

    pub fn show(&mut self, ctx: &egui::Context, keys: &mut KeyStore, profiles: &ProfileStore) -> KeyOutcome {
        let mut outcome = self.poll_job(keys);
        if self.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        let now = ctx.input(|i| i.time);
        let opened = *self.opened_at.get_or_insert(now);
        let t = (((now - opened) / theme::anim_time(0.2).max(1e-3) as f64) as f32).clamp(0.0, 1.0);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let screen = ctx.content_rect();
        let size = Vec2::new((screen.width() - 64.0).clamp(560.0, 900.0), (screen.height() - 80.0).clamp(380.0, 580.0));
        let modal = egui::Modal::new(egui::Id::new("key_manager")).frame(egui::Frame::NONE).show(ctx, |ui| {
            ui.multiply_opacity(1.0 - (1.0 - t).powi(3));
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let p = ui.painter();
            p.add(egui::Shadow { offset: [0, 16], blur: 44, spread: 0, color: theme::shadow() }.as_shape(rect, CornerRadius::same(16)));
            p.rect(rect, CornerRadius::same(16), theme::surface(), Stroke::new(1.0, theme::border().gamma_multiply(0.7)), egui::StrokeKind::Inside);
            let list = Rect::from_min_max(rect.min, Pos2::new(rect.left() + LIST_W, rect.bottom()));
            p.rect_filled(list.shrink(0.5), CornerRadius { nw: 16, sw: 16, ne: 0, se: 0 }, theme::bg());
            p.vline(list.right(), list.y_range(), Stroke::new(1.0, theme::border().gamma_multiply(0.45)));
            let close_rect = Rect::from_center_size(Pos2::new(rect.right() - 26.0, rect.top() + 26.0), Vec2::splat(28.0));
            if ui.scope_builder(egui::UiBuilder::new().max_rect(close_rect), |ui| w::close_button(ui, 28.0, false)).inner.clicked() {
                outcome = KeyOutcome::Close;
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(list.shrink2(Vec2::new(14.0, 16.0))), |ui| self.list(ui, keys, profiles, &mut outcome));
            let body = Rect::from_min_max(Pos2::new(list.right() + 26.0, rect.top() + 20.0), rect.max - Vec2::new(54.0, 18.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 14, top: 0, bottom: 8 }).show(ui, |ui| {
                        if matches!(self.panel, Panel::Generate(_)) {
                            self.generate_panel(ui);
                        } else {
                            self.details(ui, keys, profiles, &mut outcome);
                        }
                    });
                });
            });
        });
        if modal.should_close() && matches!(outcome, KeyOutcome::Open) && self.pending.is_none() {
            outcome = KeyOutcome::Close;
        }
        outcome
    }

    fn list(&mut self, ui: &mut Ui, keys: &mut KeyStore, profiles: &ProfileStore, outcome: &mut KeyOutcome) {
        ui.label(RichText::new("SSH keys").size(18.0).strong());
        ui.label(RichText::new("Generate, import and reuse keys across hosts").size(11.5).color(theme::text_dim()));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if w::primary_button(ui, "+ Generate").clicked() {
                self.panel = Panel::Generate(GenForm::default());
                self.form = Form::None;
                self.message = None;
            }
            if w::ghost_button(ui, "Import…", theme::text()).on_hover_text("Add an existing OpenSSH private key file").clicked()
                && let Some(path) = rfd::FileDialog::new().set_title("Import private key").pick_file()
            {
                match keys.import_file(&path) {
                    Ok(meta) => {
                        self.select(meta.id);
                        self.message = Some((true, format!("Imported “{}”", meta.name)));
                        if self.for_editor {
                            *outcome = KeyOutcome::Created(meta.id);
                        }
                    }
                    Err(e) => self.message = Some((false, format!("{e:#}"))),
                }
            }
        });
        ui.add_space(12.0);
        egui::ScrollArea::vertical().id_salt("key_list").auto_shrink([false, false]).show(ui, |ui| {
            if keys.keys.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No keys yet").color(theme::text_dim()));
                    ui.label(RichText::new("Generate one or import ~/.ssh/id_*").size(11.5).color(theme::text_faint()));
                });
            }
            for k in &keys.keys {
                let uses = profiles.profiles.iter().filter(|p| uses_key(p, k.id)).count();
                let selected = self.selected == Some(k.id) && matches!(self.panel, Panel::Details);
                if key_card(ui, k, uses, selected).clicked() {
                    self.select(k.id);
                }
                ui.add_space(4.0);
            }
        });
    }

    fn select(&mut self, id: Uuid) {
        self.selected = Some(id);
        self.panel = Panel::Details;
        self.form = Form::None;
        self.install_host = None;
    }

    fn details(&mut self, ui: &mut Ui, keys: &mut KeyStore, profiles: &ProfileStore, outcome: &mut KeyOutcome) {
        let Some(k) = self.selected.and_then(|id| keys.get(id)).cloned() else {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("🔑").size(34.0));
                ui.label(RichText::new("Select a key, or generate a new one").color(theme::text_dim()));
            });
            self.message_line(ui);
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&k.name).size(20.0).strong());
            tag(ui, &k.algorithm, theme::blue());
            if k.encrypted {
                tag(ui, "passphrase", theme::warn());
            }
        });
        ui.label(RichText::new(if k.comment.is_empty() { "no comment".to_string() } else { k.comment.clone() }).size(12.0).color(theme::text_faint()));
        ui.add_space(10.0);

        w::card(ui, Some("Public key"), |ui| {
            let mut text = k.public.clone();
            ui.add(egui::TextEdit::multiline(&mut text).font(egui::TextStyle::Monospace).desired_rows(3).desired_width(f32::INFINITY).interactive(true));
            ui.horizontal(|ui| {
                if w::primary_button(ui, "Copy public key").clicked() {
                    ui.ctx().copy_text(k.public.clone());
                    self.message = Some((true, "Public key copied — paste it into ~/.ssh/authorized_keys".into()));
                }
                if w::ghost_button(ui, "Save .pub…", theme::text()).clicked()
                    && let Some(path) = rfd::FileDialog::new().set_file_name(format!("{}.pub", file_stem(&k.name))).save_file()
                {
                    self.message = Some(match std::fs::write(&path, format!("{}\n", k.public)) {
                        Ok(()) => (true, format!("Saved {}", path.display())),
                        Err(e) => (false, format!("Saving failed: {e}")),
                    });
                }
            });
        });
        ui.add_space(8.0);
        w::card(ui, Some("Fingerprint"), |ui| {
            ui.horizontal_top(|ui| {
                if let Some(art) = k.randomart() {
                    egui::Frame::new().fill(theme::surface_3()).corner_radius(8).inner_margin(8).show(ui, |ui| {
                        ui.label(RichText::new(art).monospace().size(10.5).color(theme::text_dim()));
                    });
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new(&k.fingerprint).monospace().size(12.0));
                    if ui.small_button("Copy fingerprint").clicked() {
                        ui.ctx().copy_text(k.fingerprint.clone());
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("Created {}", date(k.created))).size(11.5).color(theme::text_faint()));
                    let users: Vec<&str> = profiles.profiles.iter().filter(|p| uses_key(p, k.id)).map(|p| p.display_name()).collect();
                    let used = if users.is_empty() { "Not used by any host yet".to_string() } else { format!("Used by {}", users.join(", ")) };
                    ui.add(egui::Label::new(RichText::new(used).size(11.5).color(theme::text_dim())).wrap());
                });
            });
        });
        ui.add_space(8.0);
        w::card(ui, Some("Use on a server"), |ui| self.install(ui, &k, profiles, outcome));
        ui.add_space(8.0);
        self.actions(ui, &k, keys, profiles);
        self.message_line(ui);
    }

    fn install(&mut self, ui: &mut Ui, k: &KeyMeta, profiles: &ProfileStore, outcome: &mut KeyOutcome) {
        ui.label(RichText::new("Appends the public key to ~/.ssh/authorized_keys over a session to that host.").size(11.5).color(theme::text_dim()));
        ui.horizontal(|ui| {
            let chosen = self.install_host.and_then(|id| profiles.get(id)).map(|p| p.display_name().to_string()).unwrap_or_else(|| "Choose a host…".into());
            egui::ComboBox::from_id_salt("key_install_host").selected_text(chosen).width(220.0).show_ui(ui, |ui| {
                for p in &profiles.profiles {
                    ui.selectable_value(&mut self.install_host, Some(p.id), format!("{}  ·  {}", p.display_name(), p.address()));
                }
            });
            let ready = self.install_host.is_some();
            if ui.add_enabled(ready, egui::Button::new("Install key")).clicked()
                && let Some(host) = self.install_host
            {
                *outcome = KeyOutcome::Install { key: k.id, host };
                self.message = Some((true, "Installing…".into()));
            }
        });
    }

    fn actions(&mut self, ui: &mut Ui, k: &KeyMeta, keys: &mut KeyStore, profiles: &ProfileStore) {
        ui.horizontal_wrapped(|ui| {
            if w::ghost_button(ui, "Edit", theme::text()).clicked() {
                self.form = Form::Edit { name: k.name.clone(), comment: k.comment.clone(), pass: String::new() };
            }
            let pass_label = if k.encrypted { "Change passphrase" } else { "Add passphrase" };
            if w::ghost_button(ui, pass_label, theme::text()).clicked() {
                self.form = Form::Passphrase { old: String::new(), new: String::new(), new2: String::new(), remember: secret::get(k.id).is_some() };
            }
            let export_tip = if k.encrypted {
                "Copy the private key file (stays encrypted with its passphrase)"
            } else {
                "Copy the private key file. It has NO passphrase: anyone with the copy can log in as you"
            };
            if w::ghost_button(ui, "Export private…", if k.encrypted { theme::text() } else { theme::warn() }).on_hover_text(export_tip).clicked()
                && let Some(path) = rfd::FileDialog::new().set_file_name(file_stem(&k.name)).save_file()
            {
                self.message = Some(match keys.export_private(k.id, &path) {
                    Ok(()) if k.encrypted => (true, format!("Exported to {}", path.display())),
                    Ok(()) => (true, format!("Exported to {} — unencrypted: keep that file private", path.display())),
                    Err(e) => (false, format!("{e:#}")),
                });
            }
            if w::ghost_button(ui, "Delete", theme::danger()).clicked() {
                self.form = Form::Delete;
            }
        });
        ui.add_space(6.0);
        if self.pending.is_some() && matches!(self.form, Form::Edit { .. } | Form::Passphrase { .. }) {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                loading::mini_spinner(ui, r.center(), 7.0, theme::accent());
                ui.label(RichText::new("Re-encrypting the key…").color(theme::text_dim()));
            });
            return;
        }
        let mut done = false;
        let (mut msg, mut reselect, mut job) = (None, false, None);
        match &mut self.form {
            Form::None => {}
            Form::Edit { name, comment, pass } => {
                w::card(ui, Some("Edit key"), |ui| {
                    labelled(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(name).desired_width(260.0)));
                    labelled(ui, "Comment", |ui| ui.add(egui::TextEdit::singleline(comment).desired_width(260.0)));
                    if k.encrypted && *comment != k.comment {
                        labelled(ui, "Passphrase", |ui| {
                            ui.add(egui::TextEdit::singleline(pass).password(true).hint_text("needed to change the comment").desired_width(260.0))
                        });
                    }
                    ui.horizontal(|ui| {
                        if w::primary_button(ui, "Save").clicked() {
                            let pass = Some(pass.as_str()).filter(|p| !p.is_empty()).map(str::to_string).or_else(|| secret::get(k.id));
                            match keys.prepare_edit(k.id, comment, pass) {
                                // Only the label changed: instant.
                                Ok(None) => {
                                    keys.rename(k.id, name);
                                    msg = Some(match keys.save() {
                                        Ok(()) => (true, "Key updated".into()),
                                        Err(e) => (false, format!("{e:#}")),
                                    });
                                    done = true;
                                }
                                Ok(Some(work)) => {
                                    let (id, name) = (k.id, name.clone());
                                    job = Some(spawn(move || {
                                        let rewrite = work()?;
                                        Ok(Done::Rewritten { rewrite, id, name: Some(name), remember: Remember::Keep, note: "Key updated".into() })
                                    }));
                                    done = true;
                                }
                                Err(e) => msg = Some((false, format!("{e:#}"))),
                            }
                        }
                        if w::ghost_button(ui, "Cancel", theme::text()).clicked() {
                            done = true;
                        }
                    });
                });
            }
            Form::Passphrase { old, new, new2, remember } => {
                w::card(ui, Some(if k.encrypted { "Change passphrase" } else { "Add passphrase" }), |ui| {
                    if k.encrypted {
                        labelled(ui, "Current", |ui| ui.add(egui::TextEdit::singleline(old).password(true).desired_width(240.0)));
                    }
                    labelled(ui, "New", |ui| ui.add(egui::TextEdit::singleline(new).password(true).hint_text("empty = no passphrase").desired_width(240.0)));
                    labelled(ui, "Repeat", |ui| ui.add(egui::TextEdit::singleline(new2).password(true).desired_width(240.0)));
                    strength(ui, new);
                    ui.checkbox(remember, "Remember in Windows Credential Manager");
                    let mismatch = new != new2;
                    if mismatch && !new2.is_empty() {
                        ui.label(RichText::new("Passphrases don't match").size(11.5).color(theme::danger()));
                    }
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!mismatch, egui::Button::new(RichText::new("Apply").color(theme::on_accent())).fill(theme::accent())).clicked() {
                            let old_pass = Some(old.as_str()).filter(|p| !p.is_empty()).map(str::to_string).or_else(|| secret::get(k.id));
                            let (id, path, new_pass) = (k.id, keys.key_file(k.id), new.clone());
                            let remember = match (*remember, new_pass.is_empty()) {
                                (true, false) => Remember::Store(new_pass.clone()),
                                _ => Remember::Forget,
                            };
                            let note = if new_pass.is_empty() { "Passphrase removed" } else { "Passphrase changed" }.to_string();
                            job = Some(spawn(move || {
                                let rewrite = keystore::prepare_rewrite(&path, id, old_pass.as_deref(), Some(&new_pass), None)?;
                                Ok(Done::Rewritten { rewrite, id, name: None, remember, note })
                            }));
                            done = true;
                        }
                        if w::ghost_button(ui, "Cancel", theme::text()).clicked() {
                            done = true;
                        }
                    });
                });
            }
            Form::Delete => {
                let users = profiles.profiles.iter().filter(|p| uses_key(p, k.id)).count();
                w::card(ui, Some("Delete key"), |ui| {
                    let warn = if users > 0 {
                        format!("{users} host{} use this key and will stop authenticating with it.", if users == 1 { "" } else { "s" })
                    } else {
                        "The private key file is deleted. This can't be undone.".into()
                    };
                    ui.label(RichText::new(warn).color(theme::danger()));
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("Delete for good").color(Color32::WHITE)).fill(theme::danger().gamma_multiply(0.75)))
                            .clicked()
                        {
                            match keys.remove(k.id) {
                                Ok(()) => {
                                    msg = Some((true, format!("Deleted “{}”", k.name)));
                                    reselect = true;
                                }
                                Err(e) => msg = Some((false, format!("{e:#}"))),
                            }
                            done = true;
                        }
                        if w::ghost_button(ui, "Cancel", theme::text()).clicked() {
                            done = true;
                        }
                    });
                });
            }
        }
        if let Some(m) = msg {
            self.message = Some(m);
        }
        if reselect {
            self.selected = keys.keys.first().map(|k| k.id);
        }
        if job.is_some() {
            self.pending = job;
            self.message = None;
        }
        if done {
            self.form = Form::None;
        }
    }

    fn generate_panel(&mut self, ui: &mut Ui) {
        let busy = self.pending.is_some();
        ui.label(RichText::new("Generate a key").size(20.0).strong());
        ui.label(RichText::new("The private key never leaves this PC; give servers the public key.").size(12.0).color(theme::text_dim()));
        ui.add_space(10.0);
        let mut start = false;
        if let Panel::Generate(f) = &mut self.panel {
            ui.add_enabled_ui(!busy, |ui| {
                w::card(ui, Some("Key"), |ui| {
                    labelled(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut f.name).hint_text("e.g. Laptop 2026").desired_width(280.0)));
                    labelled(ui, "Type", |ui| {
                        egui::ComboBox::from_id_salt("gen_kind")
                            .selected_text(f.kind.label())
                            .width(280.0)
                            .show_ui(ui, |ui| {
                                for kind in KeyKind::ALL {
                                    ui.selectable_value(&mut f.kind, kind, kind.label()).on_hover_text(kind.hint());
                                }
                            })
                            .response
                    });
                    ui.label(RichText::new(f.kind.hint()).size(11.5).color(theme::text_faint()));
                    labelled(ui, "Comment", |ui| ui.add(egui::TextEdit::singleline(&mut f.comment).desired_width(280.0)));
                });
                ui.add_space(8.0);
                w::card(ui, Some("Passphrase"), |ui| {
                    labelled(ui, "Passphrase", |ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.pass).password(true).hint_text("optional, recommended").desired_width(280.0))
                    });
                    labelled(ui, "Repeat", |ui| ui.add(egui::TextEdit::singleline(&mut f.pass2).password(true).desired_width(280.0)));
                    strength(ui, &f.pass);
                    if f.pass != f.pass2 && !f.pass2.is_empty() {
                        ui.label(RichText::new("Passphrases don't match").size(11.5).color(theme::danger()));
                    }
                    if !f.pass.is_empty() {
                        ui.checkbox(&mut f.remember, "Remember in Windows Credential Manager (no prompt when connecting)");
                    }
                });
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let valid = f.pass == f.pass2;
                if busy {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    loading::mini_spinner(ui, r.center(), 7.0, theme::accent());
                    ui.label(RichText::new(format!("Generating {}…", f.kind.label())).color(theme::text_dim()));
                } else if ui
                    .add_enabled(
                        valid,
                        egui::Button::new(RichText::new("Generate key").color(theme::on_accent()).strong())
                            .fill(theme::accent())
                            .min_size(Vec2::new(130.0, 30.0)),
                    )
                    .clicked()
                {
                    start = true;
                }
            });
        }
        if start {
            self.start_generate();
        }
        self.message_line(ui);
    }

    fn start_generate(&mut self) {
        let Panel::Generate(f) = &self.panel else { return };
        let form = f.clone();
        // RSA 4096 takes seconds and encrypting runs the KDF; the form shows a spinner.
        self.pending = Some(spawn(move || {
            let key = keystore::generate(form.kind, &form.comment)?;
            let key = keystore::encrypt(key, Some(form.pass.as_str()))?;
            Ok(Done::Generated { form, key: Box::new(key) })
        }));
        self.message = None;
    }

    /// Applies a finished worker job to the store (fast file writes only).
    fn poll_job(&mut self, keys: &mut KeyStore) -> KeyOutcome {
        let Some(job) = &self.pending else { return KeyOutcome::Open };
        let Some(result) = job.lock().ok().and_then(|mut s| s.take()) else { return KeyOutcome::Open };
        self.pending = None;
        match result {
            Ok(Done::Generated { form, key }) => {
                let name = if form.name.trim().is_empty() { format!("{} key", form.kind.label()) } else { form.name.clone() };
                match keys.add_key(&name, *key) {
                    Ok(meta) => {
                        let mut note = format!("Generated “{}”", meta.name);
                        if form.remember
                            && !form.pass.is_empty()
                            && let Err(e) = secret::set(meta.id, &form.pass)
                        {
                            note = format!("{note} (couldn't remember the passphrase: {e:#})");
                        }
                        self.select(meta.id);
                        self.message = Some((true, note));
                        if self.for_editor {
                            return KeyOutcome::Created(meta.id);
                        }
                    }
                    Err(e) => self.message = Some((false, format!("{e:#}"))),
                }
            }
            Ok(Done::Rewritten { rewrite, id, name, remember, note }) => {
                let applied = keys.apply(rewrite).and_then(|()| {
                    if let Some(n) = &name {
                        keys.rename(id, n);
                        keys.save()?;
                    }
                    Ok(())
                });
                self.message = Some(match applied {
                    Err(e) => (false, format!("{e:#}")),
                    Ok(()) => {
                        let stored = match remember {
                            Remember::Keep => Ok(()),
                            Remember::Store(p) => secret::set(id, &p),
                            Remember::Forget => secret::delete(id),
                        };
                        match stored {
                            Ok(()) => (true, note),
                            Err(e) => (false, format!("{note}, but the keyring failed: {e:#}")),
                        }
                    }
                });
            }
            Err(e) => self.message = Some((false, format!("{e:#}"))),
        }
        KeyOutcome::Open
    }

    fn message_line(&self, ui: &mut Ui) {
        if let Some((ok, text)) = &self.message {
            ui.add_space(6.0);
            let col = if *ok { theme::accent() } else { theme::danger() };
            ui.add(egui::Label::new(RichText::new(text).size(12.0).color(col)).wrap());
        }
    }

    /// Shows the result of an install started from here.
    pub fn report(&mut self, ok: bool, text: String) {
        self.message = Some((ok, text));
    }
}

/// Runs `work` on a worker thread; the UI polls the returned slot.
fn spawn(work: impl FnOnce() -> anyhow::Result<Done> + Send + 'static) -> Job {
    let slot: Job = Arc::default();
    let out = slot.clone();
    std::thread::spawn(move || {
        let result = work();
        if let Ok(mut s) = out.lock() {
            *s = Some(result);
        }
    });
    slot
}

fn uses_key(p: &Profile, key: Uuid) -> bool {
    matches!(p.auth, AuthMethod::Managed { key: k } if k == key)
}

fn key_card(ui: &mut Ui, k: &KeyMeta, uses: usize, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), CARD_H), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &k.name));
    let ctx = ui.ctx();
    let h = ctx.animate_bool_with_time(resp.id, resp.hovered(), theme::anim_time(0.12));
    let s = ctx.animate_bool_with_time(resp.id.with("sel"), selected, theme::anim_time(0.18));
    let p = ui.painter();
    let fill = theme::surface_2().gamma_multiply((0.5 * h).max(s));
    p.rect_filled(rect, CornerRadius::same(10), fill);
    if s > 0.0 {
        let bar = Rect::from_center_size(Pos2::new(rect.left() + 1.5, rect.center().y), Vec2::new(3.0, 22.0 * s));
        p.rect_filled(bar, CornerRadius::same(2), theme::accent());
    }
    let icon = Rect::from_center_size(Pos2::new(rect.left() + 24.0, rect.center().y), Vec2::splat(30.0));
    p.rect_filled(icon, CornerRadius::same(8), theme::accent().gamma_multiply(0.16));
    key_glyph(p, icon.shrink(7.0), theme::accent());
    if k.encrypted {
        let c = Pos2::new(icon.right() - 2.0, icon.bottom() - 2.0);
        p.circle_filled(c, 6.0, theme::bg());
        lock_glyph(p, c, theme::warn());
    }
    let x = icon.right() + 10.0;
    let clip = p.with_clip_rect(Rect::from_min_max(Pos2::new(x, rect.top()), rect.max - Vec2::new(8.0, 0.0)));
    clip.text(Pos2::new(x, rect.top() + 18.0), Align2::LEFT_CENTER, &k.name, FontId::proportional(13.5), theme::text());
    let short: String = k.fingerprint.trim_start_matches("SHA256:").chars().take(14).collect();
    let sub = format!("{} · {short}…{}", k.algorithm, if uses > 0 { format!(" · {uses} host{}", if uses == 1 { "" } else { "s" }) } else { String::new() });
    clip.text(Pos2::new(x, rect.top() + 38.0), Align2::LEFT_CENTER, sub, FontId::proportional(11.0), theme::text_faint());
    resp
}

fn key_glyph(p: &egui::Painter, r: Rect, col: Color32) {
    let s = Stroke::new(1.6, col);
    let ring = Pos2::new(r.left() + r.width() * 0.3, r.center().y);
    p.circle_stroke(ring, r.width() * 0.26, s);
    let shaft_end = Pos2::new(r.right(), r.center().y);
    p.line_segment([ring + Vec2::new(r.width() * 0.26, 0.0), shaft_end], s);
    p.line_segment([shaft_end - Vec2::new(r.width() * 0.12, 0.0), shaft_end - Vec2::new(r.width() * 0.12, -r.height() * 0.22)], s);
    p.line_segment([shaft_end - Vec2::new(r.width() * 0.32, 0.0), shaft_end - Vec2::new(r.width() * 0.32, -r.height() * 0.16)], s);
}

fn lock_glyph(p: &egui::Painter, c: Pos2, col: Color32) {
    let body = Rect::from_center_size(c + Vec2::new(0.0, 1.0), Vec2::new(6.0, 4.5));
    p.rect_filled(body, CornerRadius::same(1), col);
    p.circle_stroke(c + Vec2::new(0.0, -1.5), 2.0, Stroke::new(1.1, col));
}

fn tag(ui: &mut Ui, text: &str, col: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(11.0), col);
    let (rect, _) = ui.allocate_exact_size(galley.size() + Vec2::new(14.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(7), col.gamma_multiply(0.15));
    ui.painter().galley(rect.min + Vec2::new(7.0, 3.0), galley, col);
}

fn labelled<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(96.0, 22.0), Sense::hover());
        ui.painter().text(Pos2::new(rect.left(), rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.5), theme::text_dim());
        add(ui)
    })
    .inner
}

/// Rough passphrase strength bar (length + character classes).
fn strength(ui: &mut Ui, pass: &str) {
    if pass.is_empty() {
        return;
    }
    let classes = [
        pass.chars().any(|c| c.is_lowercase()),
        pass.chars().any(|c| c.is_uppercase()),
        pass.chars().any(|c| c.is_ascii_digit()),
        pass.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let score = ((pass.chars().count() as f32 / 16.0).min(1.0) * 0.7 + classes as f32 * 0.075).min(1.0);
    let (label, col) = match score {
        s if s < 0.4 => ("weak", theme::danger()),
        s if s < 0.7 => ("okay", theme::warn()),
        _ => ("strong", theme::accent()),
    };
    ui.horizontal(|ui| {
        ui.add_space(100.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(180.0, 4.0), Sense::hover());
        let k = ui.ctx().animate_value_with_time(resp.id, score, theme::anim_time(0.2));
        ui.painter().rect_filled(rect, CornerRadius::same(2), theme::surface_3());
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(rect.width() * k, rect.height())), CornerRadius::same(2), col);
        ui.label(RichText::new(label).size(11.0).color(col));
    });
}

/// A file name derived from the key name.
fn file_stem(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if s.trim_matches('_').is_empty() { "id_tether".into() } else { format!("id_{}", s.to_lowercase()) }
}

/// `2026-10-17` from unix seconds (UTC).
fn date(unix: u64) -> String {
    if unix == 0 {
        return "unknown".into();
    }
    // Howard Hinnant's civil-from-days.
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_file_names() {
        assert_eq!(date(0), "unknown");
        assert_eq!(date(1_760_659_200), "2025-10-17");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(file_stem("My Laptop!"), "id_my_laptop_");
        assert_eq!(file_stem("***"), "id_tether");
    }

    #[test]
    fn renders_headless_with_and_without_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut keys = KeyStore::load(dir.path().join("keys")).unwrap();
        let profiles = ProfileStore::default();
        let ctx = egui::Context::default();
        let frame = |m: &mut KeyManager, keys: &mut KeyStore| {
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                m.show(ui.ctx(), keys, &profiles);
            });
            out.textures_delta.clear();
        };
        let mut m = KeyManager::new(&keys);
        frame(&mut m, &mut keys);
        keys.add_generated("k", keystore::generate(KeyKind::Ed25519, "a@b").unwrap(), None).unwrap();
        let mut m = KeyManager::new(&keys);
        frame(&mut m, &mut keys);
        let mut g = KeyManager::generate_for_editor(&keys);
        frame(&mut g, &mut keys);
    }
}
