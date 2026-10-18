//! App-wide command bus. Widgets deep in the tree (or background tasks) push commands;
//! the app drains them once per frame and applies them to the dock.

use std::sync::{Arc, Mutex};

use crate::profile::Profile;
use crate::ssh::sftp::Sftp;

/// A remote text file ready to be shown in an editor tab.
pub struct EditorDoc {
    pub profile: Profile,
    /// Absolute remote path.
    pub remote_path: String,
    /// File contents as downloaded (decoded by the editor).
    pub bytes: Vec<u8>,
    /// Session used to save the file back.
    pub sftp: Arc<Sftp>,
    /// Remote pipeline size (KiB) of the host.
    pub pipeline_kb: u32,
}

pub enum AppCommand {
    /// Open (or focus, if already open) an editor tab for the document.
    OpenEditor(Box<EditorDoc>),
    /// Close the dock tab with this uid (an editor confirmed it may close).
    CloseTab(u64),
}

static QUEUE: Mutex<Vec<AppCommand>> = Mutex::new(Vec::new());

pub fn push(cmd: AppCommand) {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push(cmd);
}

/// Takes every queued command, oldest first.
pub fn drain() -> Vec<AppCommand> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_drains_in_order_and_empties() {
        // The queue is global; other tests never push CloseTab with these ids.
        push(AppCommand::CloseTab(9_000_001));
        push(AppCommand::CloseTab(9_000_002));
        let ids: Vec<u64> = drain()
            .into_iter()
            .filter_map(|c| match c {
                AppCommand::CloseTab(id) if id >= 9_000_000 => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, [9_000_001, 9_000_002]);
        assert!(drain().iter().all(|c| !matches!(c, AppCommand::CloseTab(id) if *id >= 9_000_000)));
    }
}
