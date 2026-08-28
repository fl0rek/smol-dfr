use std::cell::RefCell;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::eventfd::{create_eventfd, drain_eventfd, signal_eventfd};
use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::{Facility, InterestMaskSet};
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::standard::Mainloop;
use pulse::volume::Volume;

#[derive(Clone, Debug)]
pub struct VolumeState {
    pub volume_percent: u32,
    pub muted: bool,
}

struct SharedState {
    volume: VolumeState,
    changed: bool,
    connected: bool,
}

pub struct VolumeManager {
    state: Arc<Mutex<SharedState>>,
    event_fd: OwnedFd,
    thread: Mutex<Option<JoinHandle<()>>>,
    pulse_server: Option<String>,
}

impl VolumeManager {
    /// Create a new VolumeManager in disconnected state.
    /// Always succeeds -- does NOT attempt to connect.
    /// Call `try_connect()` to establish the PulseAudio connection.
    pub fn new(pulse_server: Option<&str>) -> Self {
        let event_fd = create_eventfd();
        let state = Arc::new(Mutex::new(SharedState {
            volume: VolumeState {
                volume_percent: 0,
                muted: false,
            },
            changed: false,
            connected: false,
        }));

        Self {
            state,
            event_fd,
            thread: Mutex::new(None),
            pulse_server: pulse_server.map(|s| s.to_string()),
        }
    }

    /// Attempt to connect to PulseAudio.
    /// Spawns a background thread running the PA mainloop.
    /// Returns true on successful connection, false on failure.
    pub fn try_connect(&self) -> bool {
        // Join old thread if any
        {
            let mut thread_guard = self.thread.lock().unwrap();
            if let Some(old_thread) = thread_guard.take() {
                let _ = old_thread.join();
            }
        }

        let thread_efd = unsafe { OwnedFd::from_raw_fd(libc::dup(self.event_fd.as_raw_fd())) };
        let thread_state = Arc::clone(&self.state);
        let server = self.pulse_server.clone();
        let (tx, rx) = mpsc::sync_channel(1);

        let thread = thread::spawn(move || {
            run_pa_loop(server.as_deref(), thread_state, thread_efd, tx);
        });

        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => {
                {
                    let mut s = self.state.lock().unwrap();
                    s.connected = true;
                    s.changed = true;
                }
                signal_eventfd(self.event_fd.as_raw_fd());
                *self.thread.lock().unwrap() = Some(thread);
                eprintln!("PulseAudio volume: connected");
                true
            }
            _ => {
                eprintln!("PulseAudio volume: failed to connect");
                false
            }
        }
    }

    pub fn poll(&self) -> bool {
        drain_eventfd(&self.event_fd);
        let mut state = self.state.lock().unwrap();
        let changed = state.changed;
        state.changed = false;
        changed
    }

    pub fn volume(&self) -> VolumeState {
        self.state.lock().unwrap().volume.clone()
    }

    pub fn event_fd(&self) -> BorrowedFd<'_> {
        self.event_fd.as_fd()
    }

    /// Whether the manager is currently connected to PulseAudio.
    pub fn is_connected(&self) -> bool {
        self.state.lock().unwrap().connected
    }
}

fn run_pa_loop(
    server: Option<&str>,
    state: Arc<Mutex<SharedState>>,
    event_fd: OwnedFd,
    ready_tx: mpsc::SyncSender<bool>,
) {
    let mut mainloop = match Mainloop::new() {
        Some(ml) => ml,
        None => {
            let _ = ready_tx.send(false);
            return;
        }
    };

    let context = match Context::new(&mainloop, "smol-dfr") {
        Some(ctx) => Rc::new(RefCell::new(ctx)),
        None => {
            let _ = ready_tx.send(false);
            return;
        }
    };

    if context
        .borrow_mut()
        .connect(server, ContextFlagSet::NOFLAGS, None)
        .is_err()
    {
        let _ = ready_tx.send(false);
        return;
    }

    // Iterate until context is Ready
    loop {
        match context.borrow().get_state() {
            ContextState::Ready => break,
            ContextState::Failed | ContextState::Terminated => {
                let _ = ready_tx.send(false);
                return;
            }
            _ => {
                mainloop.iterate(true);
            }
        }
    }

    let efd_raw = event_fd.as_raw_fd();

    // Set up subscribe callback
    {
        let ctx = Rc::clone(&context);
        let st = Arc::clone(&state);
        context
            .borrow_mut()
            .set_subscribe_callback(Some(Box::new(move |facility, _op, _idx| match facility {
                Some(Facility::Sink) | Some(Facility::Server) => {
                    query_volume(&ctx, &st, efd_raw);
                }
                _ => {}
            })));
    }

    context
        .borrow_mut()
        .subscribe(InterestMaskSet::SINK | InterestMaskSet::SERVER, |_| {});

    // Initial query
    query_volume(&context, &state, efd_raw);

    let _ = ready_tx.send(true);

    // Event loop -- iterate blocks until an event is dispatched
    loop {
        mainloop.iterate(true);
        match context.borrow().get_state() {
            ContextState::Failed | ContextState::Terminated => {
                eprintln!("PulseAudio volume: disconnected");
                // Clear state on disconnect
                let mut s = state.lock().unwrap();
                s.connected = false;
                s.volume = VolumeState {
                    volume_percent: 0,
                    muted: false,
                };
                s.changed = true;
                drop(s);
                signal_eventfd(efd_raw);
                break;
            }
            _ => {}
        }
    }
}

fn query_volume(context: &Rc<RefCell<Context>>, state: &Arc<Mutex<SharedState>>, efd_raw: i32) {
    let ctx_inner = Rc::clone(context);
    let state_inner = Arc::clone(state);

    context.borrow().introspect().get_server_info(move |info| {
        if let Some(ref default_sink) = info.default_sink_name {
            let name = default_sink.to_string();
            let st = Arc::clone(&state_inner);
            ctx_inner
                .borrow()
                .introspect()
                .get_sink_info_by_name(&name, move |result| {
                    if let ListResult::Item(sink_info) = result {
                        let vol_pct = (sink_info.volume.avg().0 as f64 / Volume::NORMAL.0 as f64
                            * 100.0)
                            .round() as u32;
                        let mut s = st.lock().unwrap();
                        s.volume.volume_percent = vol_pct;
                        s.volume.muted = sink_info.mute;
                        s.changed = true;
                        drop(s);
                        signal_eventfd(efd_raw);
                    }
                });
        }
    });
}
