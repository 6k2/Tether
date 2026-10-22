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

### Terminal and agent shortcuts

- **Shift+Enter** inserts a line without submitting in Claude Code, Codex and integrated PowerShell, Bash, Zsh and Fish prompts. Enter submits normally.
- **Images**: use **+ Image**, paste a screenshot or drop PNG/JPEG files onto a terminal. Colored **[Image 1]** labels show a thumbnail on hover. Tether inserts a usable file path for the shell/agent; SSH terminals upload the image over SFTP first into a private directory under the remote user's home. Local temporary copies are removed when the terminal closes. Remote copies remain available to the agent. **Clear previews** only hides the thumbnails; edit the prompt in the terminal to remove an inserted path.
- **Claude Code / Codex**: Codex has its own movable sidebar view with terminal sessions, activity, attention badges, CPU / memory, last messages and navigation, using the same monitoring system as Claude. Configure Claude hooks and Codex completion notifications in **Settings → AI agents**. Codex `notify` reports completed turns; ongoing work is tracked from terminal activity and processes.
- **Custom Boot**: right-click either agent icon, enter your local command and choose its shell. Tether passes the command and flags through unchanged, including account selectors implemented by your own functions or aliases. Choose the shell whose startup profile defines them (Windows PowerShell and PowerShell 7 use different profiles). The shell is remembered separately for each agent.

### Share machine setups and find SSH keys

Open **Import / export setup…** from the app or Hosts menu. **Export setup** previews the machines and explains that anyone with the JSON file can import their names, IP addresses / hostnames, ports, usernames and colors. Keys, key paths, passwords and passphrases are excluded. Choose the **Tether setup** source card under **Import**, review the selected machines and import them; existing endpoints are skipped and imported machines need credentials before connecting.

**Search this PC** in Import finds Bitvise profiles in common OS-provided folders. **Search this PC for SSH keys…** in the key manager or host editor searches `.ssh`, Bitvise locations, Documents, Desktop and Downloads, and displays each result's full path. Searches run in the background with a depth/time/file limit and skip symbolic links. You choose what to import; **Browse…** handles other locations. Export keys held in Bitvise's encrypted internal store as OpenSSH files from Bitvise before importing them.

## Install

Download `Tether-Setup-<version>.exe` from [Releases](https://github.com/izLoki/Tether/releases) and run it. No admin rights needed. It adds Start menu and desktop shortcuts. Tether checks for new releases on startup and installs them in one click (*Settings → About → Updates*). Running a newer installer also upgrades in place.

Windows is the main target. Other platforms compile, but they are untested.

### Build from source

```sh
git clone https://github.com/izLoki/Tether
cd Tether
cargo build --release   # → target/release/tether.exe
```

Requires Rust 1.85+ (edition 2024).

## Develop

```sh
cargo test                               # unit + headless UI tests (screenshots in target/ui-shots/)
TETHER_E2E=1 cargo test -- --test-threads=1   # against a real server, see tests/e2e_ssh.rs
```

To cut a release, bump `version` in `Cargo.toml`, commit, then run `git tag v0.1.2 && git push --tags`. GitHub Actions builds the installer (`installer/tether.iss`, Inno Setup) and publishes it with its `.sha256` and a portable exe. The icon comes from `python assets/make_icon.py`.

Config lives in `%APPDATA%\tether\tether\config\`. Set `TETHER_CONFIG_DIR` to move it.

## License

[MIT](LICENSE)
