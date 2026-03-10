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

    /// Whether the backend is currently connected to its service.
    fn is_connected(&self) -> bool;

    /// Attempt to connect (or reconnect) to the service.
    /// Returns true on success.
    fn try_connect(&self) -> bool;

    /// Check and clear the reconnect flash flag.
    /// Returns true once after a successful reconnection.
    fn has_reconnect_flash(&self) -> bool;
}

pub struct WorkspaceManager {
    backend: Box<dyn WorkspaceBackend>,
}

impl WorkspaceManager {
    /// Create a workspace manager. Always succeeds, starting in disconnected state.
    /// The manager will connect when `try_connect()` is called.
    pub fn new(_provider: Option<&str>) -> Self {
        // Niri is the only supported compositor; always create NiriBackend.
        Self {
            backend: Box::new(niri::NiriBackend::new()),
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

    /// Whether the backend is currently connected to its service.
    pub fn is_connected(&self) -> bool {
        self.backend.is_connected()
    }

    /// Attempt to connect (or reconnect) to the service.
    pub fn try_connect(&self) -> bool {
        self.backend.try_connect()
    }

    /// Check and clear the reconnect flash flag.
    pub fn has_reconnect_flash(&self) -> bool {
        self.backend.has_reconnect_flash()
    }
}
