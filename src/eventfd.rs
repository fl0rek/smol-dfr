use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

pub fn create_eventfd() -> OwnedFd {
    let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
    assert!(fd >= 0, "eventfd() failed");
    unsafe { OwnedFd::from_raw_fd(fd) }
}

pub fn signal_eventfd(fd: RawFd) {
    let val: u64 = 1;
    unsafe {
        libc::write(fd, &val as *const u64 as *const libc::c_void, 8);
    }
}

pub fn drain_eventfd(fd: &OwnedFd) {
    let mut val: u64 = 0;
    unsafe {
        libc::read(fd.as_raw_fd(), &mut val as *mut u64 as *mut libc::c_void, 8);
    }
}
