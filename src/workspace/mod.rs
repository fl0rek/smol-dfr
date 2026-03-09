pub mod niri;

use std::os::fd::BorrowedFd;

#[derive(Clone, Debug)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub idx: u8,
    pub name: Option<String>,
    pub is_active: bool,
    pub is_focused: bool,
    pub is_urgent: bool,
}

pub(crate) trait WorkspaceBackend: Send {
    /// Drain notification fd and check if state changed.
    fn poll(&self) -> bool;

    /// Get current workspace list.
    fn workspaces(&self) -> Vec<WorkspaceInfo>;

    /// Request focus on a workspace by id.
    fn focus_workspace(&self, id: u64);

    /// Get the title of the currently focused window, if any.
    fn focused_window_title(&self) -> Option<String>;

    /// Fd to register with epoll (readable when state may have changed).
    fn event_fd(&self) -> BorrowedFd<'_>;
}

pub struct WorkspaceManager {
    backend: Box<dyn WorkspaceBackend>,
}

impl WorkspaceManager {
    /// Try to create a workspace manager based on provider hint and environment.
    /// Returns None if no suitable provider is available.
    pub fn try_new(provider: Option<&str>) -> Option<Self> {
        match provider.unwrap_or("auto") {
            "niri" => niri::NiriBackend::try_new()
                .map(|b| Self { backend: Box::new(b) }),
            "auto" => {
                if std::env::var("NIRI_SOCKET").is_ok() {
                    niri::NiriBackend::try_new()
                        .map(|b| Self { backend: Box::new(b) })
                } else {
                    None
                }
            }
            other => {
                eprintln!("Unknown workspace provider: {other}");
                None
            }
        }
    }

    pub fn poll(&self) -> bool {
        self.backend.poll()
    }

    pub fn workspaces(&self) -> Vec<WorkspaceInfo> {
        self.backend.workspaces()
    }

    pub fn focus_workspace(&self, id: u64) {
        self.backend.focus_workspace(id);
    }

    pub fn focused_window_title(&self) -> Option<String> {
        self.backend.focused_window_title()
    }

    pub fn event_fd(&self) -> BorrowedFd<'_> {
        self.backend.event_fd()
    }
}
