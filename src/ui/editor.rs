//! Host editor modal: a header that previews the host, then General / Connection /
//! Authentication cards with inline validation, a collapsible Advanced section and a
//! "Test connection" button that runs a real handshake in the background.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use uuid::Uuid;

use super::autocomplete::AutoField;
use super::loading;
use super::settings_widgets as w;
use super::suggest;
use crate::folders::FolderStore;
use crate::keystore::KeyStore;
use crate::profile::{AuthMethod, MAX_PIPELINE_KB, MIN_PIPELINE_KB, Profile, secret};
use crate::ssh::connection::Stage;
use crate::theme;

const WIDTH: f32 = 500.0;
const FIELD_W: f32 = 300.0;
/// Accent presets offered next to the hue slider.
const SWATCHES: [f32; 8] = [150.0, 200.0, 220.0, 265.0, 300.0, 340.0, 20.0, 45.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthTab {
    Password,
    Managed,
    File,
}

/// Progress of a "Test connection" run, written by the background task.
#[derive(Debug, Clone, PartialEq)]
pub enum TestState {
    Idle,
    Running(Stage),
    Ok(u128),
    Failed(String),
}

pub type TestSlot = Arc<Mutex<TestState>>;

pub struct ProfileEditor {
    pub profile: Profile,
    secret: String,
    show_secret: bool,
    has_saved_secret: bool,
    is_new: bool,
    /// Validation runs on save; afterwards errors update live.
    tried_save: bool,
    auth_tab: AuthTab,
    /// Name typed for a folder to create on save.
    new_folder: Option<String>,
    users: Vec<String>,
    hosts: Vec<String>,
    user_ac: AutoField,
    host_ac: AutoField,
    test: TestSlot,
}

pub enum EditorOutcome {
    Open,
    Cancel,
    Save(Box<SaveRequest>),
    /// Open the key manager on "generate"; the new key is then selected here.
    GenerateKey,
    ManageKeys,
    /// Try to connect with these settings (result goes into the editor's test slot).
    Test(Box<Profile>, Option<String>, TestSlot),
}

pub struct SaveRequest {
    pub profile: Profile,
    /// New secret (None = keep whatever is stored).
    pub secret: Option<String>,
    /// Create this top-level folder and put the host in it.
    pub new_folder: Option<String>,
}

impl ProfileEditor {
    pub fn new(profile: Profile, is_new: bool) -> Self {
        let has_saved_secret = !is_new && secret::get(profile.secret_id()).is_some();
        let auth_tab = match profile.auth {
            AuthMethod::Password => AuthTab::Password,
            AuthMethod::Managed { .. } => AuthTab::Managed,
            AuthMethod::Key { .. } => AuthTab::File,
        };
        Self {
            profile,
            secret: String::new(),
            show_secret: false,
            has_saved_secret,
            is_new,
            tried_save: false,
            auth_tab,
            new_folder: None,
            users: Vec::new(),
            hosts: Vec::new(),
            user_ac: AutoField::new(egui::Id::new("editor_user_ac")),
            host_ac: AutoField::new(egui::Id::new("editor_host_ac")),
            test: Arc::new(Mutex::new(TestState::Idle)),
        }
    }

    /// Offers these usernames and hosts (most used first) while typing.
    pub fn with_suggestions(mut self, users: Vec<String>, hosts: Vec<String>) -> Self {
        self.users = users;
        self.hosts = hosts;
        self
    }

    /// [`Self::with_suggestions`] from the saved profiles.
    pub fn with_profiles(self, profiles: &[Profile]) -> Self {
        let (users, hosts) = suggest::profile_words(profiles);
        self.with_suggestions(users, hosts)
    }

    /// Selects a managed key (e.g. one just generated from this editor).
    pub fn use_key(&mut self, key: Uuid) {
        self.profile.auth = AuthMethod::Managed { key };
        self.auth_tab = AuthTab::Managed;
        self.has_saved_secret = secret::get(key).is_some();
    }

    pub fn show(&mut self, ctx: &egui::Context, folders: &FolderStore, keys: &KeyStore) -> EditorOutcome {
        let mut outcome = EditorOutcome::Open;
        let modal = egui::Modal::new(egui::Id::new("profile_editor")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 6.0;
            self.header(ui);
            ui.add_space(10.0);
            egui::ScrollArea::vertical().max_height(ctx.content_rect().height() - 200.0).auto_shrink([false, true]).show(ui, |ui| {
                w::card(ui, Some("General"), |ui| self.general(ui, folders));
                ui.add_space(8.0);
                w::card(ui, Some("Connection"), |ui| self.connection(ui));
                ui.add_space(8.0);
                w::card(ui, Some("Authentication"), |ui| self.auth(ui, keys, &mut outcome));
                ui.add_space(4.0);
                egui::CollapsingHeader::new(RichText::new("Advanced").color(theme::text_dim())).show(ui, |ui| self.advanced(ui));
            });
            ui.add_space(10.0);
            self.footer(ui, keys, &mut outcome);
        });
        if modal.should_close() && matches!(outcome, EditorOutcome::Open) {
            outcome = EditorOutcome::Cancel;
        }
        outcome
    }

    /// Live preview: avatar in the host colour, name and address.
    fn header(&self, ui: &mut Ui) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 48.0), Sense::hover());
        let p = ui.painter();
        let accent = theme::hue_color(self.profile.hue);
        let avatar = Rect::from_min_size(rect.left_top() + Vec2::new(0.0, 4.0), Vec2::splat(40.0));
        p.rect_filled(avatar, CornerRadius::same(10), accent.gamma_multiply(0.22));
        let name = self.profile.display_name();
        let initial = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "+".into());
        p.text(avatar.center(), Align2::CENTER_CENTER, initial, FontId::proportional(18.0), accent);
        let title = if self.is_new { "New host" } else { "Edit host" };
        p.text(Pos2::new(avatar.right() + 12.0, avatar.top() + 11.0), Align2::LEFT_CENTER, title, FontId::proportional(18.0), theme::text());
        let sub = if self.profile.host.trim().is_empty() { "Fill in the address below".to_string() } else { self.profile.address() };
        p.text(Pos2::new(avatar.right() + 12.0, avatar.bottom() - 9.0), Align2::LEFT_CENTER, sub, FontId::monospace(11.5), theme::text_faint());
    }

    fn general(&mut self, ui: &mut Ui, folders: &FolderStore) {
        field(ui, "Label", None, |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.profile.name).hint_text("My server").desired_width(FIELD_W));
        });
        field(ui, "Folder", None, |ui| self.folder_picker(ui, folders));
    }

    fn folder_picker(&mut self, ui: &mut Ui, folders: &FolderStore) {
        if let Some(name) = &mut self.new_folder {
            let cancel = ui
                .horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(name).hint_text("New folder name").desired_width(FIELD_W - 64.0));
                    ui.small_button("Cancel").clicked()
                })
                .inner;
            if cancel {
                self.new_folder = None;
            }
            return;
        }
        let current = match self.profile.folder.and_then(|f| folders.get(f)) {
            Some(_) => folders.path_of(self.profile.folder.unwrap_or_default()),
            None => "Top level".into(),
        };
        egui::ComboBox::from_id_salt("editor_folder").selected_text(current).width(FIELD_W).show_ui(ui, |ui| {
            ui.selectable_value(&mut self.profile.folder, None, "Top level");
            for (id, depth) in folders.flattened() {
                let name = folders.get(id).map(|f| f.name.as_str()).unwrap_or_default();
                ui.selectable_value(&mut self.profile.folder, Some(id), format!("{}📁 {name}", "    ".repeat(depth)));
            }
            ui.separator();
            if ui.selectable_label(false, "+ New folder…").clicked() {
                self.new_folder = Some(String::new());
            }
        });
    }

    fn connection(&mut self, ui: &mut Ui) {
        let host_err = self.tried_save && self.profile.host.trim().is_empty();
        let user_err = self.tried_save && self.profile.user.trim().is_empty();
        let (hosts, users) = (&self.hosts, &self.users);
        let p = &mut self.profile;
        field(ui, "Host", host_err.then_some("Host is required"), |ui| {
            ui.horizontal(|ui| {
                let out = self.host_ac.show(
                    ui,
                    &mut p.host,
                    |t| egui::TextEdit::singleline(t).hint_text("example.com or user@host:port").desired_width(FIELD_W - 76.0),
                    |q| suggest::rank_words(q, hosts, suggest::SUGGEST_LIMIT),
                    egui::Order::Tooltip,
                );
                // Pasting `user@host:port` fills all three fields.
                if out.changed()
                    && p.host.contains('@')
                    && let Some(q) = Profile::parse_quick(&p.host)
                {
                    p.host = q.host;
                    p.user = q.user;
                    p.port = q.port;
                }
                ui.add(egui::DragValue::new(&mut p.port).range(1..=65535).prefix("port "));
            });
        });
        field(ui, "Username", user_err.then_some("Username is required"), |ui| {
            self.user_ac.show(
                ui,
                &mut p.user,
                |t| egui::TextEdit::singleline(t).hint_text("root").desired_width(FIELD_W),
                |q| suggest::rank_words(q, users, suggest::SUGGEST_LIMIT),
                egui::Order::Tooltip,
            );
        });
    }

    fn auth(&mut self, ui: &mut Ui, keys: &KeyStore, outcome: &mut EditorOutcome) {
        let before = self.auth_tab;
        ui.horizontal(|ui| {
            w::segmented(ui, &mut self.auth_tab, &[(AuthTab::Password, "Password"), (AuthTab::Managed, "Saved key"), (AuthTab::File, "Key file")]);
        });
        if self.auth_tab != before {
            self.secret.clear();
            self.profile.auth = match self.auth_tab {
                AuthTab::Password => AuthMethod::Password,
                AuthTab::Managed => AuthMethod::Managed { key: keys.keys.first().map(|k| k.id).unwrap_or_default() },
                AuthTab::File => AuthMethod::Key { path: default_key_path() },
            };
            self.has_saved_secret = !self.is_new && secret::get(self.profile.secret_id()).is_some();
        }
        ui.add_space(6.0);
        match self.auth_tab {
            AuthTab::Password => self.secret_field(ui, "Password", ""),
            AuthTab::Managed => self.managed_key(ui, keys, outcome),
            AuthTab::File => {
                let missing = self.tried_save && matches!(&self.profile.auth, AuthMethod::Key { path } if path.as_os_str().is_empty());
                if let AuthMethod::Key { path } = &mut self.profile.auth {
                    field(ui, "Key file", missing.then_some("Choose a private key file"), |ui| {
                        ui.horizontal(|ui| {
                            let mut text = path.display().to_string();
                            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(FIELD_W - 70.0)).changed() {
                                *path = PathBuf::from(text);
                            }
                            if ui.button("Browse…").clicked()
                                && let Some(f) = rfd::FileDialog::new().set_title("Choose private key").pick_file()
                            {
                                *path = f;
                            }
                        });
                    });
                }
                self.secret_field(ui, "Passphrase", "optional");
            }
        }
    }

    fn managed_key(&mut self, ui: &mut Ui, keys: &KeyStore, outcome: &mut EditorOutcome) {
        if keys.keys.is_empty() {
            ui.horizontal(|ui| {
                ui.label(RichText::new("No saved keys yet.").color(theme::text_dim()));
                if w::primary_button(ui, "+ Generate a key").clicked() {
                    *outcome = EditorOutcome::GenerateKey;
                }
                if w::ghost_button(ui, "Import…", theme::text()).clicked() {
                    *outcome = EditorOutcome::ManageKeys;
                }
            });
            return;
        }
        let AuthMethod::Managed { key } = &mut self.profile.auth else { return };
        if keys.get(*key).is_none() {
            *key = keys.keys[0].id;
        }
        let selected = keys.get(*key).map(|k| format!("{}  ·  {}", k.name, k.algorithm)).unwrap_or_default();
        let mut changed = false;
        field(ui, "Key", None, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("editor_key").selected_text(selected).width(FIELD_W - 92.0).show_ui(ui, |ui| {
                    for k in &keys.keys {
                        let label = format!("{}{}  ·  {}", if k.encrypted { "🔒 " } else { "" }, k.name, k.algorithm);
                        changed |= ui.selectable_value(key, k.id, label).on_hover_text(&k.fingerprint).changed();
                    }
                });
                if ui.button("+").on_hover_text("Generate a new key").clicked() {
                    *outcome = EditorOutcome::GenerateKey;
                }
                if ui.button("Manage").clicked() {
                    *outcome = EditorOutcome::ManageKeys;
                }
            });
        });
        if changed {
            self.secret.clear();
            self.has_saved_secret = secret::get(self.profile.secret_id()).is_some();
        }
        let id = self.profile.secret_id();
        if let Some(k) = keys.get(id) {
            ui.label(RichText::new(&k.fingerprint).monospace().size(11.0).color(theme::text_faint()));
            if k.encrypted {
                self.secret_field(ui, "Passphrase", "asked when connecting if left blank");
            }
        }
    }

    fn secret_field(&mut self, ui: &mut Ui, label: &str, hint: &str) {
        let hint = if self.has_saved_secret { "•••••• saved — leave blank to keep" } else { hint };
        field(ui, label, None, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.secret).password(!self.show_secret).hint_text(hint).desired_width(FIELD_W - 34.0));
                let eye = if self.show_secret { "🙈" } else { "👁" };
                if ui.add(egui::Button::new(eye).frame(false)).on_hover_text("Show / hide").clicked() {
                    self.show_secret = !self.show_secret;
                }
            });
        });
    }

    fn advanced(&mut self, ui: &mut Ui) {
        let p = &mut self.profile;
        field(ui, "Remote start dir", None, |ui| {
            ui.add(egui::TextEdit::singleline(&mut p.remote_start_dir).hint_text("home (when there is no last folder)").desired_width(FIELD_W));
        });
        field(ui, "SFTP pipeline", None, |ui| {
            ui.add(egui::Slider::new(&mut p.pipeline_kb, MIN_PIPELINE_KB..=MAX_PIPELINE_KB).logarithmic(true).suffix(" KB"))
                .on_hover_text("Bytes kept in flight per file (Bitvise-style). Raise on high-latency links.");
        });
        field(ui, "Colour", None, |ui| {
            ui.horizontal(|ui| {
                for hue in SWATCHES {
                    if swatch(ui, hue, (p.hue - hue).abs() < 1.0).clicked() {
                        p.hue = hue;
                    }
                }
                ui.add(egui::Slider::new(&mut p.hue, 0.0..=359.0).show_value(false));
            });
        });
    }

    fn footer(&mut self, ui: &mut Ui, keys: &KeyStore, outcome: &mut EditorOutcome) {
        let state = self.test.lock().map(|s| s.clone()).unwrap_or(TestState::Idle);
        ui.horizontal(|ui| {
            let running = matches!(state, TestState::Running(_));
            let test = ui.add_enabled(!running, egui::Button::new("Test connection").min_size(Vec2::new(120.0, 30.0)));
            if test.clicked() {
                match self.validated(keys) {
                    Ok(()) => {
                        let secret = (!self.secret.is_empty()).then(|| self.secret.clone()).or_else(|| secret::get(self.profile.secret_id()));
                        *outcome = EditorOutcome::Test(Box::new(self.profile.clone()), secret, self.test.clone());
                    }
                    Err(e) => {
                        if let Ok(mut s) = self.test.lock() {
                            *s = TestState::Failed(e);
                        }
                    }
                }
            }
            test_status(ui, &state);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save = ui.add(
                    egui::Button::new(RichText::new("Save").color(Color32::WHITE).strong())
                        .fill(theme::accent().gamma_multiply(0.85))
                        .min_size(Vec2::new(90.0, 30.0)),
                );
                let save = save.on_hover_text("Ctrl+Enter");
                if ui.add(egui::Button::new("Cancel").min_size(Vec2::new(84.0, 30.0))).clicked() {
                    *outcome = EditorOutcome::Cancel;
                }
                if save.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl) {
                    self.tried_save = true;
                    if self.validated(keys).is_ok() {
                        let secret = (!self.secret.is_empty()).then(|| self.secret.clone());
                        let new_folder = self.new_folder.clone().filter(|n| !n.trim().is_empty());
                        *outcome = EditorOutcome::Save(Box::new(SaveRequest { profile: self.profile.clone(), secret, new_folder }));
                    }
                }
            });
        });
        if let TestState::Failed(e) = &state {
            ui.add(egui::Label::new(RichText::new(e).size(11.5).color(theme::danger())).wrap());
        }
    }

    fn validated(&self, keys: &KeyStore) -> Result<(), String> {
        self.profile.validate()?;
        if let AuthMethod::Managed { key } = self.profile.auth
            && keys.get(key).is_none()
        {
            return Err("Choose a saved key (or generate one)".into());
        }
        Ok(())
    }
}

/// Label column + control, with an optional error line under the control.
fn field(ui: &mut Ui, label: &str, error: Option<&str>, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(110.0, 24.0), Sense::hover());
        let col = if error.is_some() { theme::danger() } else { theme::text_dim() };
        ui.painter().text(Pos2::new(rect.left(), rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(13.0), col);
        ui.vertical(|ui| {
            control(ui);
            if let Some(e) = error {
                ui.label(RichText::new(e).size(11.0).color(theme::danger()));
            }
        });
    });
}

fn swatch(ui: &mut Ui, hue: f32, selected: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || selected, theme::anim_time(0.1));
    let p = ui.painter();
    p.circle_filled(rect.center(), 6.5 + h, theme::hue_color(hue));
    if selected {
        p.circle_stroke(rect.center(), 9.0, Stroke::new(1.5, theme::text()));
    }
    resp
}

fn test_status(ui: &mut Ui, state: &TestState) {
    match state {
        TestState::Idle | TestState::Failed(_) => {}
        TestState::Running(stage) => {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            loading::mini_spinner(ui, rect.center(), 6.0, theme::accent());
            ui.label(RichText::new(format!("{}…", stage.label())).size(12.0).color(theme::text_dim()));
        }
        TestState::Ok(ms) => {
            ui.label(RichText::new(format!("✔ Connected in {ms} ms")).size(12.0).color(theme::accent()));
        }
    }
}

fn default_key_path() -> PathBuf {
    let Some(dirs) = directories::UserDirs::new() else { return PathBuf::new() };
    let ssh = dirs.home_dir().join(".ssh");
    ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| ssh.join(n)).find(|p| p.exists()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasting_quick_connect_is_parsed_once_saved() {
        let mut e = ProfileEditor::new(Profile::new(), true);
        e.profile.host = "h".into();
        e.profile.user = "u".into();
        let keys = KeyStore::default();
        assert!(e.validated(&keys).is_ok());
        e.profile.auth = AuthMethod::Managed { key: Uuid::new_v4() };
        assert!(e.validated(&keys).is_err(), "unknown managed key");
        assert_eq!(e.profile.port, crate::profile::DEFAULT_PORT);
    }

    #[test]
    fn use_key_switches_to_saved_key_tab() {
        let mut e = ProfileEditor::new(Profile::new(), true);
        let id = Uuid::new_v4();
        e.use_key(id);
        assert_eq!(e.auth_tab, AuthTab::Managed);
        assert_eq!(e.profile.auth, AuthMethod::Managed { key: id });
    }
}
