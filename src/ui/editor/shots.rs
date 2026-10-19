//! Screenshots of the host editor and the key manager in a dark and a light theme.
//! Needs a GPU adapter: `cargo test --lib form_screens -- --ignored`; PNGs land in
//! `target/ui-shots/form-*.png`.

use eframe::egui::{self, Vec2};

use super::{AuthTab, ProfileEditor};
use crate::folders::FolderStore;
use crate::keystore::{self, KeyKind, KeyStore};
use crate::profile::{AuthMethod, Profile, ProfileStore};
use crate::ui::key_manager::KeyManager;

const THEMES: [(&str, &str); 2] = [("night", "Tether Night"), ("day", "Tether Day")];

fn out_dir() -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/ui-shots");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn render(name: &str, theme: &str, mut frame: impl FnMut(&egui::Context)) {
    let theme = theme.to_string();
    let mut themed = false;
    let mut h = egui_kittest::Harness::builder().with_size(Vec2::new(1360.0, 1180.0)).wgpu().build_ui(move |ui| {
        if !themed {
            crate::themes::apply(ui.ctx(), &theme);
            themed = true;
        }
        ui.painter().rect_filled(ui.max_rect(), 0.0, crate::theme::bg());
        frame(ui.ctx());
    });
    for _ in 0..30 {
        h.step();
    }
    h.render().expect("render").save(out_dir().join(format!("form-{name}.png"))).unwrap();
}

fn keys_with_two() -> (tempfile::TempDir, KeyStore) {
    let dir = tempfile::tempdir().unwrap();
    let mut keys = KeyStore::load(dir.path().join("keys")).unwrap();
    keys.add_generated("Laptop 2026", keystore::generate(KeyKind::Ed25519, "ana@laptop").unwrap(), None).unwrap();
    let enc = keystore::encrypt(keystore::generate(KeyKind::EcdsaP256, "deploy@ci").unwrap(), Some("pw")).unwrap();
    keys.add_key("CI deploy", enc).unwrap();
    (dir, keys)
}

fn editor(auth: AuthTab, keys: &KeyStore, tried: bool) -> ProfileEditor {
    let mut p = Profile::new();
    if !tried {
        p.name = "api-prod-01".into();
        p.host = "10.0.4.21".into();
        p.user = "deploy".into();
    }
    p.auth = match auth {
        AuthTab::Password => AuthMethod::Password,
        AuthTab::Managed => AuthMethod::Managed { key: keys.keys[0].id },
        AuthTab::File => AuthMethod::Key { path: "C:\\Users\\you\\.ssh\\id_ed25519".into() },
    };
    let mut e = ProfileEditor::new(p, true);
    e.tried_save = tried;
    e
}

#[test]
#[ignore]
fn form_screens() {
    let (dir, keys) = keys_with_two();
    let folders = FolderStore::default();
    let profiles = ProfileStore::default();
    for (tag, theme) in THEMES {
        for (name, auth, tried) in [("password-errors", AuthTab::Password, true), ("saved-key", AuthTab::Managed, false), ("key-file", AuthTab::File, false)] {
            let mut e = editor(auth, &keys, tried);
            render(&format!("editor-{name}-{tag}"), theme, |ctx| {
                e.show(ctx, &folders, &keys);
            });
        }
        let empty = KeyStore::default();
        let mut e = editor(AuthTab::Managed, &keys, false);
        render(&format!("editor-no-keys-{tag}"), theme, |ctx| {
            e.show(ctx, &folders, &empty);
        });

        let mut m = KeyManager::new(&keys);
        let mut k = KeyStore::load(dir.path().join("keys")).unwrap();
        render(&format!("keys-details-{tag}"), theme, |ctx| {
            m.show(ctx, &mut k, &profiles);
        });
        let mut g = KeyManager::generate_for_editor(&keys);
        render(&format!("keys-generate-{tag}"), theme, |ctx| {
            g.show(ctx, &mut k, &profiles);
        });
        let mut none = KeyStore::default();
        let mut m = KeyManager::new(&none);
        render(&format!("keys-empty-{tag}"), theme, |ctx| {
            m.show(ctx, &mut none, &profiles);
        });
    }
}
