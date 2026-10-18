//! What the sidebar, the host editor and the key manager ask the app to do.
//! A child module of `app` so it can use the app's private state.

use eframe::egui;
use uuid::Uuid;

use super::TetherApp;
use crate::profile::{Profile, secret};
use crate::ssh::connection::{Stage, exec};
use crate::ui::editor::{EditorOutcome, ProfileEditor, TestSlot, TestState};
use crate::ui::import_view::ImportDialog;
use crate::ui::key_manager::{KeyManager, KeyOutcome};
use crate::ui::sidebar::SidebarAction;

const INSTALL_OUTPUT: usize = 16 * 1024;

impl TetherApp {
    pub(super) fn apply_sidebar(&mut self, action: SidebarAction, ctx: &egui::Context) {
        match action {
            SidebarAction::Open(id, kind) => {
                if let Some(p) = self.store.get(id).cloned() {
                    self.request_open(p, kind, ctx);
                }
            }
            SidebarAction::New(folder) => self.open_editor_new(folder),
            SidebarAction::Edit(id) => self.editor = self.store.get(id).cloned().map(|p| ProfileEditor::new(p, false).with_profiles(&self.store.profiles)),
            SidebarAction::Duplicate(id) => {
                if let Some(mut p) = self.store.get(id).cloned() {
                    p.id = Uuid::new_v4();
                    p.name = format!("{} copy", p.display_name());
                    p.last_used = 0;
                    self.editor = Some(ProfileEditor::new(p, true).with_profiles(&self.store.profiles));
                }
            }
            SidebarAction::Delete(id) => self.confirm_delete = Some(id),
            SidebarAction::ForgetHostKey(id) => {
                if let Some(p) = self.store.get(id) {
                    match self.pool.forget_host_key(&p.host, p.port) {
                        Ok(()) => self.toasts.info(format!("Host key for {} forgotten", p.host)),
                        Err(e) => self.toasts.error(format!("{e:#}")),
                    }
                }
            }
            SidebarAction::QuickConnect(p) => {
                self.store.upsert(p.clone());
                self.save_profiles();
                self.request_open(p, crate::ui::sidebar::OpenKind::Terminal, ctx);
            }
            SidebarAction::SaveWorkspace => self.save_workspace = Some(String::new()),
            SidebarAction::RestoreWorkspace(name) => self.restore_workspace(&name, ctx),
            SidebarAction::DeleteWorkspace(name) => {
                let _ = self.workspaces.remove(&name);
            }
            SidebarAction::NewFolder(parent) => self.new_folder(parent),
            SidebarAction::RenameFolder(id, name) => {
                self.folders.rename(id, &name);
                self.save_folders();
            }
            SidebarAction::DeleteFolder(id) => {
                let name = self.folders.get(id).map(|f| f.name.clone()).unwrap_or_default();
                let parent = self.folders.remove(id);
                let mut moved = 0;
                for p in self.store.profiles.iter_mut().filter(|p| p.folder == Some(id)) {
                    p.folder = parent;
                    moved += 1;
                }
                self.save_folders();
                self.save_profiles();
                let target = parent.map(|p| self.folders.path_of(p)).unwrap_or_else(|| "the top level".into());
                let what = if moved == 0 { String::new() } else { format!(" · {moved} host{} moved to {target}", if moved == 1 { "" } else { "s" }) };
                self.toasts.info(format!("Folder “{name}” deleted{what}"));
            }
            SidebarAction::SetCollapsed(id, collapsed) => {
                self.folders.set_collapsed(id, collapsed);
                self.save_folders();
            }
            SidebarAction::CollapseAll(collapsed) => {
                self.folders.set_all_collapsed(collapsed);
                self.save_folders();
            }
            SidebarAction::MoveHost(id, folder) => {
                if let Some(p) = self.store.profiles.iter_mut().find(|p| p.id == id) {
                    p.folder = folder.filter(|f| self.folders.get(*f).is_some());
                }
                if let Some(f) = folder {
                    self.folders.set_collapsed(f, false);
                    self.save_folders();
                }
                self.save_profiles();
            }
            SidebarAction::MoveFolder(id, parent) => {
                if self.folders.move_to(id, parent) {
                    if let Some(p) = parent {
                        self.folders.set_collapsed(p, false);
                    }
                    self.save_folders();
                }
            }
            SidebarAction::Archive(id, archived) => {
                if let Some(p) = self.store.profiles.iter_mut().find(|p| p.id == id) {
                    p.archived = archived;
                    let verb = if archived { "archived" } else { "restored" };
                    self.toasts.info(format!("{} {verb}", p.display_name()));
                }
                self.save_profiles();
            }
            SidebarAction::SetArchiveOpen(open) => {
                self.folders.archive_open = open;
                self.save_folders();
            }
            SidebarAction::OpenKeys => self.key_manager = Some(KeyManager::new(&self.keys)),
            SidebarAction::Import => self.import = Some(ImportDialog::new(&self.store)),
        }
    }

    pub(super) fn open_editor_new(&mut self, folder: Option<Uuid>) {
        let mut p = Profile::new();
        p.folder = folder;
        self.editor = Some(ProfileEditor::new(p, true).with_profiles(&self.store.profiles));
    }

    /// Creates "New folder" inside `parent` and lets the sidebar rename it in place.
    pub(super) fn new_folder(&mut self, parent: Option<Uuid>) {
        let id = self.folders.create("New folder", parent);
        if let Some(p) = parent {
            self.folders.set_collapsed(p, false);
        }
        self.save_folders();
        self.sidebar_open = true;
        self.sidebar.start_rename(id, "New folder");
    }

    fn save_profiles(&self) {
        if let Err(e) = self.store.save() {
            self.toasts.error(format!("Could not save profiles: {e:#}"));
        }
    }

    fn save_folders(&self) {
        if let Err(e) = self.folders.save() {
            self.toasts.error(format!("Could not save folders: {e:#}"));
        }
    }

    pub(super) fn editor_modal(&mut self, ctx: &egui::Context) {
        let Some(editor) = &mut self.editor else { return };
        let outcome = editor.show(ctx, &self.folders, &self.keys);
        match outcome {
            EditorOutcome::Open => {}
            EditorOutcome::Cancel => self.editor = None,
            EditorOutcome::GenerateKey => self.key_manager = Some(KeyManager::generate_for_editor(&self.keys)),
            EditorOutcome::ManageKeys => self.key_manager = Some(KeyManager::new(&self.keys).for_editor()),
            EditorOutcome::Test(profile, secret, slot) => self.test_connection(*profile, secret, slot, ctx.clone()),
            EditorOutcome::Save(req) => {
                let mut profile = req.profile;
                if let Some(name) = req.new_folder {
                    profile.folder = Some(self.folders.create(&name, None));
                    self.save_folders();
                }
                if let Some(s) = req.secret
                    && let Err(e) = secret::set(profile.secret_id(), &s)
                {
                    self.toasts.error(format!("{e:#}"));
                }
                self.toasts.success(format!("Saved {}", profile.display_name()));
                self.store.upsert(profile);
                self.save_profiles();
                self.editor = None;
            }
        }
    }

    pub(super) fn key_manager_modal(&mut self, ctx: &egui::Context) {
        let Some(manager) = &mut self.key_manager else { return };
        match manager.show(ctx, &mut self.keys, &self.store) {
            KeyOutcome::Open => {}
            KeyOutcome::Close => self.key_manager = None,
            KeyOutcome::Created(id) => {
                if let Some(editor) = &mut self.editor {
                    editor.use_key(id);
                    self.key_manager = None;
                }
            }
            KeyOutcome::Install { key, host } => self.install_key(key, host),
        }
    }

    /// Appends a managed public key to `authorized_keys` on `host`.
    fn install_key(&mut self, key: Uuid, host: Uuid) {
        let (Some(meta), Some(profile)) = (self.keys.get(key).cloned(), self.store.get(host).cloned()) else { return };
        let command = match meta.install_command() {
            Ok(c) => c,
            Err(e) => {
                self.toasts.error(format!("{e:#}"));
                return;
            }
        };
        if self.needs_secret(&profile) {
            let msg = format!("Open a session to {} first (or save its password), then install the key", profile.display_name());
            self.report_install(false, msg);
            return;
        }
        let (pool, toasts) = (self.pool.clone(), self.toasts.clone());
        let secret = secret::get(profile.secret_id());
        self.rt.spawn(async move {
            let result = async {
                let conn = pool.get(&profile, secret).await?;
                let (_, status) = exec(&conn, command, INSTALL_OUTPUT).await?;
                anyhow::ensure!(status == Some(0), "the server answered with exit status {status:?}");
                anyhow::Ok(())
            }
            .await;
            match result {
                Ok(()) => toasts.success(format!("Key “{}” installed on {}. Pick it under Edit host → Authentication.", meta.name, profile.display_name())),
                Err(e) => toasts.error(format!("Installing the key on {} failed: {e:#}", profile.display_name())),
            }
        });
        self.report_install(
            true,
            format!("Installing on {}… (a notification confirms)", self.store.get(host).map(|p| p.display_name().to_string()).unwrap_or_default()),
        );
    }

    fn report_install(&mut self, ok: bool, text: String) {
        match &mut self.key_manager {
            Some(m) => m.report(ok, text),
            None if ok => self.toasts.info(text),
            None => self.toasts.error(text),
        }
    }

    /// The host editor's "Test connection": a one-off handshake, progress into `slot`.
    fn test_connection(&mut self, profile: Profile, secret: Option<String>, slot: TestSlot, ctx: egui::Context) {
        let pool = self.pool.clone();
        set_test(&slot, TestState::Running(Stage::Connecting));
        self.rt.spawn(async move {
            let on_stage = {
                let (slot, ctx) = (slot.clone(), ctx.clone());
                move |stage: Stage| {
                    set_test(&slot, TestState::Running(stage));
                    ctx.request_repaint();
                }
            };
            let state = match pool.probe(&profile, secret, &on_stage).await {
                Ok(took) => TestState::Ok(took.as_millis()),
                Err(e) => TestState::Failed(format!("{e:#}")),
            };
            set_test(&slot, state);
            ctx.request_repaint();
        });
    }
}

fn set_test(slot: &TestSlot, state: TestState) {
    if let Ok(mut s) = slot.lock() {
        *s = state;
    }
}
