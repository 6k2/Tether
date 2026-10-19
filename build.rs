//! Embeds the app icon into tether.exe (Explorer, taskbar, shortcuts).

fn main() {
    println!("cargo:rerun-if-changed=assets/tether.ico");
    println!("cargo:rerun-if-changed=assets/tether.rc");
    #[cfg(windows)]
    embed_resource::compile("assets/tether.rc", embed_resource::NONE).manifest_optional().expect("embedding the app icon");
}
