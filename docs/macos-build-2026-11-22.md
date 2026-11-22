# Local macOS build verification — 2026-11-22

The latest upstream release and `main` were both v0.3.0,
`317e863984172e12a9ec00a56b9ae65098dec626`, when checked against GitHub.
The macOS portability and General Updates contributions are already merged there.

This branch fixes the stale `TerminalView.typed_line` accesses in macOS tests,
an extra closing brace in the shell integration tests, the platform-specific
split shortcut assertion, formatting, and four `collapsible_if` diagnostics.
The conditional rewrites preserve the application behavior.

Verified on Apple Silicon, macOS 27.2 (26B5101f), Rust 1.99.1:

- `cargo fmt --check` and `git diff --check`: passed.
- Strict Clippy with all targets: passed after the four conditional rewrites.
- Library tests: 388 passed, 10 explicitly ignored.
- Real local PTY tests: 2 passed.
- Basic bash and zsh shell integration: 2 passed.
- SFTP prompt path test with its in-process SSH fixture: 1 passed.
- Rendered UI flow and layout tests: 2 passed; minimum-size and Retina screenshots inspected.
- Native Keychain persistence, update and deletion with disposable credentials: 1 passed.

The full shell integration suite is **not green**. The existing
`shift_enter_inserts_a_line_and_clear_removes_old_blocks` test still fails because
the completed bash block contains `echo first_line` instead of the full multiline
command. Its preceding assertion confirms that Shift-Enter itself does not submit
the command. This failure was also recorded in the October 29 build; it has not
been disabled or weakened here.

Packaging uses `scripts/package-macos.sh aarch64-apple-darwin` with the locked
dependencies and completed successfully. The DMG passed checksum and disk image
verification, was mounted read-only, and its extracted bundle passed strict code
signature verification. The executable is arm64 and links only macOS libraries.
The packaged application opened, and a native zsh tab was created during manual
QA. The new build was installed in `/Applications/Tether.app`, and its welcome
window was verified after launch. The previous application bundle was preserved
as a local backup outside Applications.

The bundle has an ad-hoc signature, not Developer ID notarization.
Intel hardware, macOS 11, external SSH/SFTP servers and the complete manual release
matrix were not tested in this run. This local verification does not certify a
successful remote CI run or a publicly notarized release.
