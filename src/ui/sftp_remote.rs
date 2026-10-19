//! Remote pane operations: navigation through the back / forward history, ops that
//! re-list afterwards, and background listings for suggestions and Tab completion.
//! A failure caused by a dropped connection hands over to the reconnect instead of
//! showing raw error text (`sftp_session.rs`).

use std::future::Future;
use std::sync::Arc;

use anyhow::Result;

use super::{Link, NavOp, Row, SftpView, cached, candidates, learn, list_rows, path_complete, set};
use crate::ssh::sftp::{self, Sftp};

impl SftpView {
    /// Runs an op against the server, then re-lists the current directory.
    pub(super) fn remote_op<F, Fut>(&self, op: F)
    where
        F: FnOnce(Arc<Sftp>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send,
    {
        let Some(sftp) = self.sftp() else { return };
        let (state, ctx, cache, host, session) = (self.remote.clone(), self.ctx.clone(), self.cache.clone(), self.profile.id, self.session());
        let (mut generation, mut cwd) = (0, String::new());
        set(&state, |s| {
            s.loading = true;
            s.generation += 1;
            generation = s.generation;
            cwd = s.cwd.clone();
        });
        self.rt.spawn(async move {
            let op_result = op(sftp.clone()).await;
            let listed = list_rows(&sftp, &cwd).await;
            if let Ok(rows) = &listed {
                learn(&cache, host, &cwd, rows, false);
            }
            // Triage outside the state lock: a lost connection updates the state itself.
            let op_error = op_result.err().and_then(|e| Self::triage(session.as_ref(), &sftp, e));
            let listed = listed.map_err(|e| Self::triage(session.as_ref(), &sftp, e));
            set(&state, |s| {
                // A reconnect in progress owns the pane state now.
                if s.generation != generation || !matches!(s.link, Link::Ready { .. }) {
                    return;
                }
                s.loading = false;
                s.error = op_error;
                s.stale = false;
                match listed {
                    Ok(rows) if s.cwd == cwd => s.rows = rows,
                    Ok(_) => {}
                    Err(e) => s.error = e.or(s.error.take()),
                }
            });
            ctx.request_repaint();
        });
    }

    /// Navigates at once (cached rows, or an empty list, under the new path) and loads the
    /// fresh listing in the background; a failure restores the previous folder and history.
    pub(super) fn remote_go(&mut self, op: NavOp) {
        let Some(sftp) = self.sftp() else { return };
        let visited = !matches!(op, NavOp::Reload);
        let cache = self.cache.clone();
        let mut plan = None;
        set(&self.remote, |s| {
            let before = (s.cwd.clone(), s.rows.clone(), s.nav.clone());
            let target = match op {
                NavOp::Visit(p) => {
                    let abs = if p.starts_with('/') { p } else { sftp::join(&s.cwd, &p) };
                    let target = path_complete::normalize_remote(&abs);
                    s.nav.visit(s.cwd.clone(), &target);
                    target
                }
                NavOp::Back => match s.nav.back(s.cwd.clone()) {
                    Some(t) => t,
                    None => return,
                },
                NavOp::Forward => match s.nav.forward(s.cwd.clone()) {
                    Some(t) => t,
                    None => return,
                },
                NavOp::Reload => s.cwd.clone(),
            };
            if target != s.cwd {
                match cached(&cache, &target) {
                    Some(rows) => {
                        s.rows = rows;
                        s.stale = false;
                    }
                    None => s.stale = !s.rows.is_empty(),
                }
                s.cwd = target.clone();
            }
            s.loading = true;
            s.error = None;
            s.generation += 1;
            plan = Some((target, s.generation, before));
        });
        let Some((target, generation, (prev_cwd, prev_rows, prev_nav))) = plan else { return };
        if visited {
            self.remote_list.clear();
        }
        let (state, ctx, host, recorder, session) = (self.remote.clone(), self.ctx.clone(), self.profile.id, self.recorder(), self.session());
        self.rt.spawn(async move {
            let canonical = sftp.canonicalize(&target).await.unwrap_or(target);
            let listed = list_rows(&sftp, &canonical).await;
            if let Ok(rows) = &listed {
                learn(&cache, host, &canonical, rows, visited);
                if visited {
                    recorder.record(&canonical);
                }
            }
            let listed = listed.map_err(|e| Self::triage(session.as_ref(), &sftp, e));
            set(&state, |s| {
                // A reconnect in progress owns the pane state now.
                if s.generation != generation || !matches!(s.link, Link::Ready { .. }) {
                    return;
                }
                s.loading = false;
                s.stale = false;
                match listed {
                    Ok(rows) => {
                        s.cwd = canonical;
                        s.rows = rows;
                    }
                    Err(None) => {}
                    Err(Some(e)) => {
                        s.error = Some(e);
                        if visited {
                            (s.cwd, s.rows, s.nav) = (prev_cwd, prev_rows, prev_nav);
                        }
                    }
                }
            });
            ctx.request_repaint();
        });
    }

    pub(super) fn remote_navigate(&mut self, path: String) {
        self.remote_go(NavOp::Visit(path));
    }

    /// Lists `dir` in the background (for suggestions) unless it is cached or in flight.
    pub(super) fn remote_prefetch(&self, dir: String) {
        if !dir.starts_with('/') {
            return;
        }
        let dir = path_complete::normalize_remote(&dir);
        if self.cache.lock().map(|c| c.listings.contains(&dir)).unwrap_or(true) {
            return;
        }
        let Some(sftp) = self.sftp() else { return };
        if !self.prefetching.lock().map(|mut p| p.insert(dir.clone())).unwrap_or(false) {
            return;
        }
        let (cache, inflight, ctx, host) = (self.cache.clone(), self.prefetching.clone(), self.ctx.clone(), self.profile.id);
        self.rt.spawn(async move {
            if let Ok(rows) = list_rows(&sftp, &dir).await {
                learn(&cache, host, &dir, &rows, false);
            }
            set(&inflight, |p| {
                p.remove(&dir);
            });
            ctx.request_repaint();
        });
    }

    /// Supplies the remote path field with a directory listing for Tab completion:
    /// the current rows, the host cache, or (no cached data) a live listing.
    pub(super) fn remote_completion_listing(&mut self, dir: String, cwd: &str, rows: &[Row]) {
        if dir == cwd || path_complete::listing_dir(cwd, path_complete::REMOTE) == dir {
            self.remote_path.provide_listing(dir, candidates(rows));
            return;
        }
        if let Some(rows) = cached(&self.cache, &path_complete::normalize_remote(&dir)) {
            self.remote_path.provide_listing(dir, candidates(&rows));
            return;
        }
        let Some(sftp) = self.sftp() else {
            self.remote_path.listing_failed(&dir);
            return;
        };
        let (slot, ctx, cache, host) = (self.completion.clone(), self.ctx.clone(), self.cache.clone(), self.profile.id);
        self.rt.spawn(async move {
            let listed = list_rows(&sftp, &dir).await.ok();
            if let Some(rows) = &listed {
                learn(&cache, host, &path_complete::normalize_remote(&dir), rows, false);
            }
            set(&slot, |s| *s = Some((dir, listed.map(|rows| candidates(&rows)))));
            ctx.request_repaint();
        });
    }
}
