# tether

SSH terminals and fast SFTP side by side, in one native Rust window.

![tether](docs/screenshot.png)

A personal project: the split-pane workspace of Termius plus the SFTP speed of Bitvise, with no Electron and no account. Built with [egui](https://github.com/emilk/egui) and [russh](https://github.com/Eugeny/russh).

## Features

- **Split workspace**: drag any terminal or SFTP tab into any split layout. Saved workspaces, broadcast input to several terminals.
- **Fast SFTP**: one SSH connection is shared by every tab, and transfers keep up to 4 MB of requests in flight. That's about 2–3× faster than a plain client at 20 ms latency.
- **Hosts**: nested folders, drag and drop, archive, search and quick connect (`user@host:port`). Imports hosts from `~/.ssh/config` and Bitvise.
- **Key manager**: generate, import and export keys, add or change a passphrase, and install a key on a server in one click.
- **SFTP that remembers**: each host reopens in its last folder, and *Go to terminal folder* follows the shell next to it.
- **Secure by default**: secrets live in the OS credential store, host keys are trusted on first use, and tether refuses to connect if a host key changes.
- **Customisable**: themes, and every shortcut can be rebound (*Settings → Keyboard*).

## Install

Download `Tether-Setup-<version>.exe` from [Releases](https://github.com/izLoki/Tether/releases) and run it. No admin rights needed. It adds Start menu and desktop shortcuts. Tether checks for new releases on startup and installs them in one click (*Settings → About → Updates*). Running a newer installer also upgrades in place.

On **macOS**, download the `Tether-<version>-macos-<arch>.dmg` for Apple Silicon
(`aarch64`) or Intel (`x86_64`), then drag Tether to Applications. macOS builds use
Command shortcuts, Keychain, Finder file copying and native login shells. Packages
currently use ad-hoc signing; Developer ID signing/notarization is still needed for
Gatekeeper-trusted distribution. See [macOS support and QA](docs/macos.md).

Windows and macOS have automated checks. Linux remains untested.

### Build from source

```sh
git clone https://github.com/izLoki/Tether
cd Tether
cargo build --release --locked   # Windows: target/release/tether.exe; macOS: target/release/tether
# macOS app bundle + disk image:
scripts/package-macos.sh
```

Requires Rust 1.95+ (edition 2024), matching egui/eframe. macOS also needs Xcode Command Line Tools.

## Develop

```sh
cargo test                               # unit + headless UI tests (screenshots in target/ui-shots/)
TETHER_E2E=1 cargo test -- --test-threads=1   # against a real server, see tests/e2e_ssh.rs
```

To cut a release, bump `version` in `Cargo.toml`, commit, then run `git tag v0.1.2 && git push --tags`. GitHub Actions builds the installer (`installer/tether.iss`, Inno Setup) and publishes it with its `.sha256` and a portable exe. macOS jobs build Apple Silicon and Intel app bundles and publish
DMGs with SHA-256 checksums. The icon comes from `python assets/make_icon.py`.

Config lives in `%APPDATA%\tether\tether\config\` on Windows and
`~/Library/Application Support/dev.tether.tether/` on macOS. Set `TETHER_CONFIG_DIR` to move it.

## License

[MIT](LICENSE)
