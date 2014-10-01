// Copyright 2013-2014 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Native thread-blocking I/O implementation
//!
//! This module contains the implementation of native thread-blocking
//! implementations of I/O on all platforms. This module is not intended to be
//! used directly, but rather the rust runtime will fall back to using it if
//! necessary.
//!
//! Rust code normally runs inside of green tasks with a local scheduler using
//! asynchronous I/O to cooperate among tasks. This model is not always
//! available, however, and that's where these native implementations come into
//! play. The only dependencies of these modules are the normal system libraries
//! that you would find on the respective platform.

#![allow(non_snake_case)]

use libc::{mod, c_int};
use std::c_str::CString;
use std::os;
use std::rt::rtio::{mod, IoResult, IoError};
use std::num;

// Local re-exports
pub use self::process::Process;

mod helper_thread;

// Native I/O implementations
pub mod addrinfo;
pub mod net;
pub mod process;
mod util;

#[cfg(any(target_os = "macos",
          target_os = "ios",
          target_os = "freebsd",
          target_os = "dragonfly",
          target_os = "android",
          target_os = "linux"))]
#[path = "timer_unix.rs"]
pub mod timer;

#[cfg(target_os = "windows")]
#[path = "timer_windows.rs"]
pub mod timer;

#[cfg(unix)]
#[path = "pipe_unix.rs"]
pub mod pipe;

#[cfg(windows)]
#[path = "pipe_windows.rs"]
pub mod pipe;

#[cfg(windows)]
#[path = "tty_windows.rs"]
mod tty;

fn unimpl() -> IoError {
    #[cfg(unix)] use libc::ENOSYS as ERROR;
    #[cfg(windows)] use libc::ERROR_CALL_NOT_IMPLEMENTED as ERROR;
    IoError {
        code: ERROR as uint,
        extra: 0,
        detail: Some("not yet supported by the `native` runtime, maybe try `green`.".to_string()),
    }
}

fn last_error() -> IoError {
    let errno = os::errno() as uint;
    IoError {
        code: os::errno() as uint,
        extra: 0,
        detail: Some(os::error_string(errno)),
    }
}

#[cfg(windows)]
#[inline]
fn retry<I> (f: || -> I) -> I { f() } // PR rust-lang/rust/#17020

#[cfg(unix)]
#[inline]
fn retry<I: PartialEq + num::One + Neg<I>> (f: || -> I) -> I {
    let minus_one = -num::one::<I>();
    loop {
        let n = f();
        if n == minus_one && os::errno() == libc::EINTR as int { }
        else { return n }
    }
}


fn keep_going(data: &[u8], f: |*const u8, uint| -> i64) -> i64 {
    let origamt = data.len();
    let mut data = data.as_ptr();
    let mut amt = origamt;
    while amt > 0 {
        let ret = retry(|| f(data, amt));
        if ret == 0 {
            break
        } else if ret != -1 {
            amt -= ret as uint;
            data = unsafe { data.offset(ret as int) };
        } else {
            return ret;
        }
    }
    return (origamt - amt) as i64;
}

/// Implementation of rt::rtio's IoFactory trait to generate handles to the
/// native I/O functionality.
pub struct IoFactory {
    _cannot_construct_outside_of_this_module: ()
}

impl IoFactory {
    pub fn new() -> IoFactory {
        net::init();
        IoFactory { _cannot_construct_outside_of_this_module: () }
    }
}

impl rtio::IoFactory for IoFactory {
    // networking
    fn tcp_connect(&mut self, addr: rtio::SocketAddr,
                   timeout: Option<u64>)
        -> IoResult<Box<rtio::RtioTcpStream + Send>>
    {
        net::TcpStream::connect(addr, timeout).map(|s| {
            box s as Box<rtio::RtioTcpStream + Send>
        })
    }
    fn tcp_bind(&mut self, addr: rtio::SocketAddr)
                -> IoResult<Box<rtio::RtioTcpListener + Send>> {
        net::TcpListener::bind(addr).map(|s| {
            box s as Box<rtio::RtioTcpListener + Send>
        })
    }
    fn udp_bind(&mut self, addr: rtio::SocketAddr)
                -> IoResult<Box<rtio::RtioUdpSocket + Send>> {
        net::UdpSocket::bind(addr).map(|u| {
            box u as Box<rtio::RtioUdpSocket + Send>
        })
    }
    fn unix_bind(&mut self, path: &CString)
                 -> IoResult<Box<rtio::RtioUnixListener + Send>> {
        pipe::UnixListener::bind(path).map(|s| {
            box s as Box<rtio::RtioUnixListener + Send>
        })
    }
    fn unix_connect(&mut self, path: &CString,
                    timeout: Option<u64>) -> IoResult<Box<rtio::RtioPipe + Send>> {
        pipe::UnixStream::connect(path, timeout).map(|s| {
            box s as Box<rtio::RtioPipe + Send>
        })
    }
    fn get_host_addresses(&mut self, host: Option<&str>, servname: Option<&str>,
                          hint: Option<rtio::AddrinfoHint>)
        -> IoResult<Vec<rtio::AddrinfoInfo>>
    {
        addrinfo::GetAddrInfoRequest::run(host, servname, hint)
    }

    // misc
    fn timer_init(&mut self) -> IoResult<Box<rtio::RtioTimer + Send>> {
        timer::Timer::new().map(|t| box t as Box<rtio::RtioTimer + Send>)
    }
    fn spawn(&mut self, cfg: rtio::ProcessConfig)
            -> IoResult<(Box<rtio::RtioProcess + Send>,
                         Vec<Option<Box<rtio::RtioPipe + Send>>>)> {
        process::Process::spawn(cfg).map(|(p, io)| {
            (box p as Box<rtio::RtioProcess + Send>,
             io.into_iter().map(|p| p.map(|p| {
                 box p as Box<rtio::RtioPipe + Send>
             })).collect())
        })
    }
    fn kill(&mut self, pid: libc::pid_t, signum: int) -> IoResult<()> {
        process::Process::kill(pid, signum)
    }
    fn pipe_open(&mut self, fd: c_int) -> IoResult<Box<rtio::RtioPipe + Send>> {
        Ok(box file::FileDesc::new(fd, true) as Box<rtio::RtioPipe + Send>)
    }
    #[cfg(unix)]
    fn tty_open(&mut self, fd: c_int, _readable: bool)
                -> IoResult<Box<rtio::RtioTTY + Send>> {
        if unsafe { libc::isatty(fd) } != 0 {
            Ok(box file::FileDesc::new(fd, true) as Box<rtio::RtioTTY + Send>)
        } else {
            Err(IoError {
                code: libc::ENOTTY as uint,
                extra: 0,
                detail: None,
            })
        }
    }
    #[cfg(windows)]
    fn tty_open(&mut self, fd: c_int, _readable: bool)
                -> IoResult<Box<rtio::RtioTTY + Send>> {
        if tty::is_tty(fd) {
            Ok(box tty::WindowsTTY::new(fd) as Box<rtio::RtioTTY + Send>)
        } else {
            Err(IoError {
                code: libc::ERROR_INVALID_HANDLE as uint,
                extra: 0,
                detail: None,
            })
        }
    }
}
