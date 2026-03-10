use std::cell::RefCell;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};

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
}

pub struct VolumeManager {
    state: Arc<Mutex<SharedState>>,
    event_fd: OwnedFd,
    _thread: JoinHandle<()>,
}

fn create_eventfd() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    assert!(fd >= 0, "eventfd() failed");
    unsafe { OwnedFd::from_raw_fd(fd) }
}

fn signal_fd(fd: i32) {
    let val: u64 = 1;
    unsafe {
        libc::write(fd, &val as *const u64 as *const libc::c_void, 8);
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

impl VolumeManager {
    pub fn try_new(pulse_server: Option<&str>) -> Option<Self> {
        let event_fd = create_eventfd();
        let thread_efd = unsafe { OwnedFd::from_raw_fd(libc::dup(event_fd.as_raw_fd())) };

        let state = Arc::new(Mutex::new(SharedState {
            volume: VolumeState {
                volume_percent: 0,
                muted: false,
            },
            changed: false,
        }));

        let thread_state = Arc::clone(&state);
        let server = pulse_server.map(|s| s.to_string());
        let (tx, rx) = mpsc::sync_channel(1);

        let thread = thread::spawn(move || {
            run_pa_loop(server.as_deref(), thread_state, thread_efd, tx);
        });

        match rx.recv() {
            Ok(true) => {
                eprintln!("PulseAudio volume: connected");
                Some(Self {
                    state,
                    event_fd,
                    _thread: thread,
                })
            }
            _ => {
                eprintln!("Failed to connect to PulseAudio");
                None
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

    let context = match Context::new(&mainloop, "tiny-dfr") {
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
        context.borrow_mut().set_subscribe_callback(Some(Box::new(
            move |facility, _op, _idx| match facility {
                Some(Facility::Sink) | Some(Facility::Server) => {
                    query_volume(&ctx, &st, efd_raw);
                }
                _ => {}
            },
        )));
    }

    context
        .borrow_mut()
        .subscribe(InterestMaskSet::SINK | InterestMaskSet::SERVER, |_| {});

    // Initial query
    query_volume(&context, &state, efd_raw);

    let _ = ready_tx.send(true);

    // Event loop — iterate blocks until an event is dispatched
    loop {
        mainloop.iterate(true);
        match context.borrow().get_state() {
            ContextState::Failed | ContextState::Terminated => {
                eprintln!("PulseAudio context disconnected");
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
                        let vol_pct = (sink_info.volume.avg().0 as f64
                            / Volume::NORMAL.0 as f64
                            * 100.0)
                            .round() as u32;
                        let mut s = st.lock().unwrap();
                        s.volume.volume_percent = vol_pct;
                        s.volume.muted = sink_info.mute;
                        s.changed = true;
                        drop(s);
                        signal_fd(efd_raw);
                    }
                });
        }
    });
}
