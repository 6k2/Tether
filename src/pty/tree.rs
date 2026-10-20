//! The whole process tree of a local shell, so closing its tab (or quitting tether) ends
//! everything it started (an agent CLI, its node workers, a dev server) and not just the
//! shell itself.
//!
//! Windows: the shell is put in a Job Object with `KILL_ON_JOB_CLOSE`; its descendants
//! join the job automatically, and the job dies with our handle to it, even when tether
//! crashes. Programs that explicitly ask to break away (`CREATE_BREAKAWAY_FROM_JOB`) may.
//! Unix: the shell leads its own session and process group (portable-pty `setsid`s it);
//! the group gets SIGHUP, which job-control shells forward to their jobs.

use portable_pty::Child;

pub struct ProcessTree {
    #[cfg(windows)]
    job: Option<windows::Job>,
    #[cfg(unix)]
    group: Option<i32>,
}

impl ProcessTree {
    /// Takes over `child`'s tree. Best effort: without a job / group, `kill` does nothing
    /// and the caller's plain kill of the shell still applies.
    pub fn adopt(child: &(dyn Child + Send + Sync)) -> Self {
        #[cfg(windows)]
        {
            Self { job: child.as_raw_handle().and_then(windows::Job::with_process) }
        }
        #[cfg(unix)]
        {
            Self { group: child.process_id().and_then(|pid| i32::try_from(pid).ok()).filter(|pid| *pid > 1) }
        }
        #[cfg(not(any(windows, unix)))]
        {
            let _ = child;
            Self {}
        }
    }

    /// Ends every process of the tree.
    pub fn kill(&self) {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            job.terminate();
        }
        #[cfg(unix)]
        if let Some(group) = self.group {
            // SAFETY: plain syscall; a negative pid addresses the process group.
            unsafe {
                libc::kill(-group, libc::SIGHUP);
            }
        }
    }

    /// Whether the tree is tracked (a job / process group was set up).
    #[cfg(test)]
    pub fn is_tracked(&self) -> bool {
        #[cfg(windows)]
        {
            self.job.is_some()
        }
        #[cfg(unix)]
        {
            self.group.is_some()
        }
        #[cfg(not(any(windows, unix)))]
        {
            false
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    };

    pub struct Job(HANDLE);

    // SAFETY: a job handle is a kernel handle usable from any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// A new kill-on-close job containing `process`.
        pub fn with_process(process: RawHandle) -> Option<Self> {
            // SAFETY: FFI with valid arguments; the job handle is owned by `Job` (closed on drop).
            unsafe {
                let job = Job(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
                if job.0.is_null() {
                    return None;
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
                let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
                if SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) == 0 {
                    return None;
                }
                if AssignProcessToJobObject(job.0, process as HANDLE) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        pub fn terminate(&self) {
            // SAFETY: `self.0` is a live job handle.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: closing the handle we own; with KILL_ON_JOB_CLOSE this ends the tree.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    fn alive(pid: u32) -> bool {
        let out = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }

    /// The shell's grandchild (a stand-in for `claude`) dies with the tree.
    #[test]
    fn killing_the_tree_ends_grandchildren() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid.txt");
        let script = format!(
            "$p = Start-Process -PassThru -NoNewWindow ping -ArgumentList '-n','60','127.0.0.1'; Set-Content -Path '{}' -Value $p.Id; Start-Sleep 60",
            pid_file.display()
        );
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let tree = ProcessTree::adopt(&child);
        assert!(tree.is_tracked());
        let started = Instant::now();
        let grandchild = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
                break pid;
            }
            if started.elapsed() > Duration::from_secs(20) {
                tree.kill();
                panic!("the shell never started its child");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(alive(grandchild));
        tree.kill();
        let _ = child.wait();
        let started = Instant::now();
        while alive(grandchild) {
            assert!(started.elapsed() < Duration::from_secs(5), "grandchild survived the tree kill");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
