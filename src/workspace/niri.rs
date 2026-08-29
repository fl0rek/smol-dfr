use super::{WorkspaceBackend, WorkspaceInfo};
use niri_ipc::socket::Socket;
use niri_ipc::{Action, Event, Request, Response, WorkspaceReferenceArg};
use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crate::eventfd::{create_eventfd, drain_eventfd, signal_eventfd};

struct SharedState {
    workspaces: Vec<WorkspaceInfo>,
    windows: HashMap<u64, Option<String>>,
    focused_window_id: Option<u64>,
    changed: bool,
    connected: bool,
}

pub struct NiriBackend {
    state: Arc<Mutex<SharedState>>,
    event_fd: OwnedFd,
    cmd_socket: Mutex<Option<Socket>>,
    reader_thread: Option<JoinHandle<()>>,
}

impl NiriBackend {
    /// Create a new NiriBackend in disconnected state.
    /// Always succeeds -- does NOT attempt to connect.
    /// Call `try_connect()` to establish the connection.
    pub fn new() -> Self {
        let event_fd = create_eventfd();
        let state = Arc::new(Mutex::new(SharedState {
            workspaces: Vec::new(),
            windows: HashMap::new(),
            focused_window_id: None,
            changed: false,
            connected: false,
        }));

        Self {
            state,
            event_fd,
            cmd_socket: Mutex::new(None),
            reader_thread: None,
        }
    }

    /// Attempt to connect to the niri socket.
    /// Re-discovers the socket path, establishes event and command connections,
    /// spawns the reader thread.
    /// Returns true on success, false on failure (logged to stderr).
    pub fn try_connect(&mut self) -> bool {
        // Discover socket path
        let uid = unsafe { libc::getuid() };
        let socket_path = match crate::session_detect::discover_niri_socket(uid) {
            Some(path) => path,
            None => {
                eprintln!("niri workspace: no socket found for uid {uid}");
                return false;
            }
        };

        // Update env var so niri_ipc::socket::Socket::connect() finds it
        // Safety: we are the only thread that calls this, and niri_ipc reads
        // NIRI_SOCKET synchronously during Socket::connect().
        unsafe { std::env::set_var("NIRI_SOCKET", &socket_path) };

        // Connect event socket
        let mut event_socket = match Socket::connect() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("niri workspace: failed to connect event socket: {e}");
                return false;
            }
        };

        // Request event stream
        match event_socket.send(Request::EventStream) {
            Ok(Ok(Response::Handled)) => {}
            Ok(Ok(other)) => {
                eprintln!("niri workspace: unexpected EventStream response: {other:?}");
                return false;
            }
            Ok(Err(msg)) => {
                eprintln!("niri workspace: EventStream error: {msg}");
                return false;
            }
            Err(e) => {
                eprintln!("niri workspace: failed to start event stream: {e}");
                return false;
            }
        }

        // Connect command socket
        let cmd_socket = match Socket::connect() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("niri workspace: failed to connect command socket: {e}");
                return false;
            }
        };

        // Store the new command socket
        *self.cmd_socket.lock().unwrap() = Some(cmd_socket);

        // Join old reader thread if any
        if let Some(old_thread) = self.reader_thread.take() {
            let _ = old_thread.join();
        }

        // Dup eventfd for the new thread
        let thread_event_fd = unsafe { OwnedFd::from_raw_fd(libc::dup(self.event_fd.as_raw_fd())) };

        // Mark connected, set flash, signal
        {
            let mut s = self.state.lock().unwrap();
            s.connected = true;
            s.changed = true;
        }
        signal_eventfd(self.event_fd.as_raw_fd());

        // Spawn new reader thread
        let thread_state = Arc::clone(&self.state);
        let reader_thread = thread::spawn(move || {
            Self::event_reader(event_socket, thread_state, thread_event_fd);
        });

        self.reader_thread = Some(reader_thread);

        eprintln!("niri workspace: connected");
        true
    }

    fn event_reader(socket: Socket, state: Arc<Mutex<SharedState>>, event_fd: OwnedFd) {
        let mut read_event = socket.read_events();
        loop {
            match read_event() {
                Ok(event) => Self::handle_event(&event, &state, &event_fd),
                Err(e) => {
                    eprintln!("niri workspace: disconnected: {e}");
                    // Clear state on disconnect
                    let mut s = state.lock().unwrap();
                    s.connected = false;
                    s.workspaces.clear();
                    s.windows.clear();
                    s.focused_window_id = None;
                    s.changed = true;
                    drop(s);
                    signal_eventfd(event_fd.as_raw_fd());
                    break;
                }
            }
        }
    }

    fn handle_event(event: &Event, state: &Arc<Mutex<SharedState>>, event_fd: &OwnedFd) {
        let mut s = state.lock().unwrap();
        let changed = match event {
            Event::WorkspacesChanged { workspaces } => {
                let mut ws_list: Vec<_> = workspaces
                    .iter()
                    .map(|ws| WorkspaceInfo {
                        id: ws.id,
                        idx: ws.idx,
                        name: ws.name.clone(),
                        is_active: ws.is_active,
                        is_focused: ws.is_focused,
                        is_urgent: ws.is_urgent,
                    })
                    .collect();
                ws_list.sort_by_key(|w| w.idx);
                s.workspaces = ws_list;
                true
            }
            Event::WorkspaceActivated { id, focused } => {
                if *focused {
                    for ws in &mut s.workspaces {
                        ws.is_focused = ws.id == *id;
                    }
                }
                if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == *id) {
                    ws.is_active = true;
                }
                true
            }
            Event::WorkspaceUrgencyChanged { id, urgent } => {
                if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == *id) {
                    ws.is_urgent = *urgent;
                }
                true
            }
            Event::WindowsChanged { windows } => {
                s.windows.clear();
                s.focused_window_id = None;
                for w in windows {
                    s.windows.insert(w.id, w.title.clone());
                    if w.is_focused {
                        s.focused_window_id = Some(w.id);
                    }
                }
                true
            }
            Event::WindowOpenedOrChanged { window } => {
                s.windows.insert(window.id, window.title.clone());
                if window.is_focused {
                    s.focused_window_id = Some(window.id);
                }
                true
            }
            Event::WindowClosed { id } => {
                s.windows.remove(id);
                if s.focused_window_id == Some(*id) {
                    s.focused_window_id = None;
                }
                true
            }
            Event::WindowFocusChanged { id } => {
                s.focused_window_id = *id;
                true
            }
            _ => false,
        };

        if changed {
            s.changed = true;
            drop(s);
            signal_eventfd(event_fd.as_raw_fd());
        }
    }
}

impl WorkspaceBackend for NiriBackend {
    fn poll(&self) -> bool {
        drain_eventfd(&self.event_fd);
        let mut state = self.state.lock().unwrap();
        let changed = state.changed;
        state.changed = false;
        changed
    }

    fn workspaces(&self) -> Vec<WorkspaceInfo> {
        self.state.lock().unwrap().workspaces.clone()
    }

    fn focus_workspace(&self, id: u64) {
        let mut socket_guard = self.cmd_socket.lock().unwrap();
        let socket = match socket_guard.as_mut() {
            Some(s) => s,
            None => return, // Disconnected -- silently ignore
        };
        match socket.send(Request::Action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(id),
        })) {
            Ok(Ok(Response::Handled)) => {}
            Ok(Err(msg)) => eprintln!("niri FocusWorkspace error: {msg}"),
            Err(e) => eprintln!("Failed to send FocusWorkspace: {e}"),
            _ => {}
        }
    }

    fn focused_window_title(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .focused_window_id
            .and_then(|id| state.windows.get(&id))
            .and_then(|title| title.clone())
    }

    fn event_fd(&self) -> BorrowedFd<'_> {
        self.event_fd.as_fd()
    }

    fn is_connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }

    fn try_connect(&mut self) -> bool {
        NiriBackend::try_connect(self)
    }
}
