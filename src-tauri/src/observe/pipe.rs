//! Windows named-pipe transport: Lattice is the server, the instrumented program
//! is the client (it simply `fopen`s the pipe path it was given).
//!
//! Properties this module is responsible for:
//! - **Live:** a reader thread feeds events to the [`Ingest`] as they arrive.
//! - **Local only:** `PIPE_REJECT_REMOTE_CLIENTS`, one instance, a per-session name.
//! - **Never hangs Lattice.** The program connects lazily (at its first event), so
//!   a program that emits nothing never connects; [`PipeServer::finish`] unblocks
//!   the pending connect by connecting a throwaway client, and waits for the
//!   reader only for a bounded time.
//! - **Never blocks the program because of us.** After a bad event the reader keeps
//!   draining (see [`Ingest`]), so the program is not stalled on a full pipe.
//!
//! Backpressure: if Lattice reads slower than the program writes, the program's
//! writes block. That is deliberate (no silent loss); the reader thread does
//! nothing but read and decode.

use std::io;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::receiver::Ingest;

/// How long [`PipeServer::finish`] waits for the stream to end after the program
/// has exited. A grandchild process that inherited the pipe could keep it open.
const FINISH_TIMEOUT: Duration = Duration::from_secs(3);

pub struct PipeServer {
    name: String,
    thread: Option<JoinHandle<()>>,
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadFile, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
        PIPE_WAIT,
    };

    pub struct Handle(pub HANDLE);
    // A pipe handle may be used from the thread that owns it.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
    }

    pub fn create(name: &str) -> io::Result<Handle> {
        let w = wide(name);
        let h = unsafe {
            CreateNamedPipeW(
                w.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                1 << 16,
                0,
                std::ptr::null(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(Handle(h))
        }
    }

    /// Block until a client connects. `false` if the wait failed.
    pub fn connect(h: &Handle) -> bool {
        unsafe { ConnectNamedPipe(h.0, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED }
    }

    /// Read into `buf`; `None` at end of stream (client closed) or on error.
    pub fn read(h: &Handle, buf: &mut [u8]) -> Option<usize> {
        let mut n: u32 = 0;
        let ok = unsafe {
            ReadFile(h.0, buf.as_mut_ptr() as *mut c_void as *mut _, buf.len() as u32, &mut n, std::ptr::null_mut())
        };
        if ok == 0 || n == 0 {
            None
        } else {
            Some(n as usize)
        }
    }

    /// Connect and immediately close a client, releasing a server blocked in
    /// `connect`. Harmless if the real client already holds the instance.
    pub fn poke(name: &str) {
        let w = wide(name);
        let h = unsafe {
            CreateFileW(w.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
        };
        if h != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(h) };
        }
    }
}

impl PipeServer {
    /// Create the pipe (so a client can connect immediately) and start reading
    /// into `ingest`. `name` is the full path, `\\.\pipe\<something unique>`.
    #[cfg(windows)]
    pub fn start(name: String, ingest: Arc<Mutex<Ingest>>) -> io::Result<Self> {
        let handle = sys::create(&name)?;
        let thread = std::thread::Builder::new().name("lattice-events".into()).spawn(move || {
            if sys::connect(&handle) {
                let mut buf = vec![0u8; 64 * 1024];
                while let Some(n) = sys::read(&handle, &mut buf) {
                    ingest.lock().unwrap().feed(&buf[..n]);
                }
            }
            // `handle` dropped here: the pipe is closed by the thread that owns it.
        })?;
        Ok(PipeServer { name, thread: Some(thread) })
    }

    #[cfg(not(windows))]
    pub fn start(_: String, _: Arc<Mutex<Ingest>>) -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "named pipes are implemented for Windows only"))
    }

    /// The program has exited: make sure the reader ends, and wait (bounded) for
    /// it. Returns `false` if the stream did not close in time (the reader is
    /// then left to finish on its own).
    pub fn finish(&mut self) -> bool {
        let Some(thread) = self.thread.take() else { return true };
        #[cfg(windows)]
        {
            // If the program never connected, the reader is still blocked in
            // `connect`; this releases it. If the program did connect, the reader
            // ends by itself when the program's handle closed.
            let deadline = Instant::now() + FINISH_TIMEOUT;
            while !thread.is_finished() {
                sys::poke(&self.name);
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = thread.join();
        true
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        // E.g. the build failed and the program never ran: do not leak the thread.
        self.finish();
    }
}
