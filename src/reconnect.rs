use nix::errno::Errno;
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify, WatchDescriptor};
use std::os::fd::{AsFd, BorrowedFd};

/// Which service socket appeared during a `check_events()` call.
#[derive(Debug, Default)]
pub struct ReconnectEvents {
    pub niri: bool,
    pub pulse: bool,
}

/// Watches socket directories via inotify and reports when niri or `PulseAudio`
/// sockets appear. Uses a single shared Inotify instance for efficiency.
pub struct ReconnectWatcher {
    inotify: Inotify,
    niri_wd: Option<WatchDescriptor>,
    pulse_wd: Option<WatchDescriptor>,
    /// Watch on `xdg_runtime_dir` for pulse directory creation (only if `pulse_dir`
    /// didn't exist at construction time).
    parent_wd: Option<WatchDescriptor>,
    niri_dir: String,
    pulse_dir: String,
}

const WATCH_FLAGS: AddWatchFlags = AddWatchFlags::IN_CREATE.union(AddWatchFlags::IN_MOVED_TO);

const DIR_WATCH_FLAGS: AddWatchFlags = AddWatchFlags::IN_CREATE.union(AddWatchFlags::IN_ISDIR);

impl ReconnectWatcher {
    /// Create a new watcher for niri and `PulseAudio` socket directories.
    ///
    /// `xdg_runtime_dir` is typically `/run/user/<uid>`.
    /// - Niri sockets appear directly in `xdg_runtime_dir` as `niri.*.sock`
    /// - `PulseAudio` socket appears at `<xdg_runtime_dir>/pulse/native`
    ///
    /// Handles missing directories gracefully (ENOENT) by deferring watch
    /// setup until the directory appears.
    pub fn new(xdg_runtime_dir: &str) -> Self {
        let inotify = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC)
            .expect("Failed to create inotify instance");

        let niri_dir = xdg_runtime_dir.to_string();
        let pulse_dir = format!("{xdg_runtime_dir}/pulse");

        let niri_wd = add_watch_safe(&inotify, &niri_dir, WATCH_FLAGS);
        let pulse_wd = add_watch_safe(&inotify, &pulse_dir, WATCH_FLAGS);

        // If the pulse directory doesn't exist, watch the parent for its creation
        let parent_wd = if pulse_wd.is_none() {
            add_watch_safe(&inotify, xdg_runtime_dir, DIR_WATCH_FLAGS)
        } else {
            None
        };

        Self {
            inotify,
            niri_wd,
            pulse_wd,
            parent_wd,
            niri_dir,
            pulse_dir,
        }
    }

    /// Check for inotify events and report which service sockets appeared.
    ///
    /// Returns a `ReconnectEvents` struct indicating which sockets were detected.
    /// Non-blocking: returns defaults (false, false) if no events are pending.
    pub fn check_events(&mut self) -> ReconnectEvents {
        let events = match self.inotify.read_events() {
            Ok(evts) => evts,
            Err(Errno::EAGAIN | Errno::EWOULDBLOCK) => return ReconnectEvents::default(),
            Err(e) => {
                eprintln!("reconnect watcher: inotify read error: {e}");
                return ReconnectEvents::default();
            }
        };

        let mut result = ReconnectEvents::default();

        for event in &events {
            // Handle IN_IGNORED (watch invalidated, e.g. directory deleted)
            if event.mask.contains(AddWatchFlags::IN_IGNORED) {
                if self.niri_wd == Some(event.wd) {
                    self.niri_wd = None;
                } else if self.pulse_wd == Some(event.wd) {
                    self.pulse_wd = None;
                    // Re-add parent watch to detect pulse dir recreation
                    if self.parent_wd.is_none() {
                        self.parent_wd = add_watch_safe(
                            &self.inotify,
                            &self.niri_dir, // niri_dir == xdg_runtime_dir
                            DIR_WATCH_FLAGS,
                        );
                    }
                } else if self.parent_wd == Some(event.wd) {
                    self.parent_wd = None;
                }
                continue;
            }

            let name = match &event.name {
                Some(n) => n.to_string_lossy(),
                None => continue,
            };

            if Some(event.wd) == self.niri_wd {
                if name.starts_with("niri.") && name.ends_with(".sock") {
                    result.niri = true;
                }
            } else if Some(event.wd) == self.pulse_wd {
                if *name == *"native" {
                    result.pulse = true;
                }
            } else if Some(event.wd) == self.parent_wd && *name == *"pulse" {
                // The pulse directory was just created; add a watch on it
                self.pulse_wd = add_watch_safe(&self.inotify, &self.pulse_dir, WATCH_FLAGS);
                // Remove parent watch -- no longer needed
                if let Some(wd) = self.parent_wd.take() {
                    let _ = self.inotify.rm_watch(wd);
                }
            }
        }

        result
    }

    /// Return the inotify fd for epoll registration.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }

    /// Re-add any missing watches. Call periodically or after handling
    /// `IN_IGNORED` events to recover from deleted directories.
    pub fn ensure_watches(&mut self) {
        if self.niri_wd.is_none() {
            self.niri_wd = add_watch_safe(&self.inotify, &self.niri_dir, WATCH_FLAGS);
        }
        if self.pulse_wd.is_none() {
            self.pulse_wd = add_watch_safe(&self.inotify, &self.pulse_dir, WATCH_FLAGS);
            // If we got pulse_wd now, remove the parent watch
            if self.pulse_wd.is_some() {
                if let Some(wd) = self.parent_wd.take() {
                    let _ = self.inotify.rm_watch(wd);
                }
            } else if self.parent_wd.is_none() {
                // Still no pulse dir; re-watch parent
                self.parent_wd = add_watch_safe(&self.inotify, &self.niri_dir, DIR_WATCH_FLAGS);
            }
        }
    }
}

/// Try to add an inotify watch, returning None on ENOENT (directory doesn't
/// exist yet) instead of panicking.
fn add_watch_safe(inotify: &Inotify, path: &str, flags: AddWatchFlags) -> Option<WatchDescriptor> {
    match inotify.add_watch(path, flags) {
        Ok(wd) => Some(wd),
        Err(Errno::ENOENT) => None,
        Err(e) => {
            eprintln!("reconnect watcher: failed to watch {path}: {e}");
            None
        }
    }
}
