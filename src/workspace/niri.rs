use super::{WorkspaceBackend, WorkspaceInfo};
use niri_ipc::{Action, Event, Request, Response, WorkspaceReferenceArg};
use niri_ipc::socket::Socket;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

struct SharedState {
    workspaces: Vec<WorkspaceInfo>,
    changed: bool,
}

pub struct NiriBackend {
    state: Arc<Mutex<SharedState>>,
    event_fd: OwnedFd,
    cmd_socket: Mutex<Socket>,
    _reader_thread: JoinHandle<()>,
}

fn create_eventfd() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    assert!(fd >= 0, "eventfd() failed");
    unsafe { OwnedFd::from_raw_fd(fd) }
}

fn signal_eventfd(fd: &OwnedFd) {
    let val: u64 = 1;
    unsafe {
        libc::write(
            fd.as_raw_fd(),
            &val as *const u64 as *const libc::c_void,
            8,
        );
    }
}

fn drain_eventfd(fd: &OwnedFd) {
    let mut val: u64 = 0;
    unsafe {
        libc::read(
            fd.as_raw_fd(),
            &mut val as *mut u64 as *mut libc::c_void,
            8,
        );
    }
}

impl NiriBackend {
    pub fn try_new() -> Option<Self> {
        let mut event_socket: Socket = match Socket::connect() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to connect to niri socket: {e}");
                return None;
            }
        };

        let cmd_socket = match Socket::connect() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to connect to niri command socket: {e}");
                return None;
            }
        };

        match event_socket.send(Request::EventStream) {
            Ok(Ok(Response::Handled)) => {}
            Ok(Ok(other)) => {
                eprintln!("Unexpected niri EventStream response: {other:?}");
                return None;
            }
            Ok(Err(msg)) => {
                eprintln!("niri EventStream error: {msg}");
                return None;
            }
            Err(e) => {
                eprintln!("Failed to start niri event stream: {e}");
                return None;
            }
        }

        let event_fd = create_eventfd();
        let thread_event_fd =
            unsafe { OwnedFd::from_raw_fd(libc::dup(event_fd.as_raw_fd())) };

        eprintln!("niri workspace: connected");

        let state = Arc::new(Mutex::new(SharedState {
            workspaces: Vec::new(),
            changed: false,
        }));

        let thread_state = Arc::clone(&state);
        let reader_thread = thread::spawn(move || {
            Self::event_reader(event_socket, thread_state, thread_event_fd);
        });

        Some(Self {
            state,
            event_fd,
            cmd_socket: Mutex::new(cmd_socket),
            _reader_thread: reader_thread,
        })
    }

    fn event_reader(
        socket: Socket,
        state: Arc<Mutex<SharedState>>,
        event_fd: OwnedFd,
    ) {
        let mut read_event = socket.read_events();
        loop {
            match read_event() {
                Ok(event) => Self::handle_event(&event, &state, &event_fd),
                Err(e) => {
                    eprintln!("niri event stream disconnected: {e}");
                    break;
                }
            }
        }
    }

    fn handle_event(
        event: &Event,
        state: &Arc<Mutex<SharedState>>,
        event_fd: &OwnedFd,
    ) {
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
            _ => false,
        };

        if changed {
            s.changed = true;
            drop(s);
            signal_eventfd(event_fd);
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
        let mut socket = self.cmd_socket.lock().unwrap();
        match socket.send(Request::Action(Action::FocusWorkspace {
            reference: WorkspaceReferenceArg::Id(id),
        })) {
            Ok(Ok(Response::Handled)) => {}
            Ok(Err(msg)) => eprintln!("niri FocusWorkspace error: {msg}"),
            Err(e) => eprintln!("Failed to send FocusWorkspace: {e}"),
            _ => {}
        }
    }

    fn event_fd(&self) -> BorrowedFd<'_> {
        self.event_fd.as_fd()
    }
}
