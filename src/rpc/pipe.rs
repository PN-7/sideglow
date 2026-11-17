// SPDX-License-Identifier: MPL-2.0
//! Windows IPC adapter. No socket, process injection, or PeekNamedPipe loop.
//!
//! Reads use overlapped I/O and block on its completion event. The 200 ms wait
//! slice only observes the caller's cancellation flag; it issues no new reads,
//! Discord commands, or network requests while an operation remains pending.

use super::{RpcError, Wire};
use std::ptr::{null, null_mut};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_BROKEN_PIPE, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
        ERROR_MORE_DATA, ERROR_NO_DATA, ERROR_OPERATION_ABORTED, ERROR_PIPE_NOT_CONNECTED,
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_ANONYMOUS,
        SECURITY_SQOS_PRESENT,
    },
    System::{
        Pipes::GetNamedPipeServerProcessId,
        Threading::{
            CreateEventW, OpenProcess, QueryFullProcessImageNameW, WaitForSingleObject,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
    },
};

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // Each wrapper uniquely owns a successfully created handle.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct Pipe {
    handle: Handle,
    cancellation: Arc<AtomicBool>,
}

// Pipe has one owner and uses &mut self for every operation. No pending I/O
// outlives a method call, and the handle is never shared with another thread.
unsafe impl Send for Pipe {}

impl Pipe {
    pub(super) fn connect(cancellation: Arc<AtomicBool>) -> Result<Self, RpcError> {
        let mut untrusted = false;
        for index in 0..10 {
            if cancellation.load(Ordering::Acquire) {
                return Err(RpcError::Cancelled);
            }
            let path: Vec<u16> = format!(r"\\?\pipe\discord-ipc-{index}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let raw = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS,
                    null_mut(),
                )
            };
            if raw == INVALID_HANDLE_VALUE {
                continue;
            }
            let handle = Handle(raw);
            // Fail closed if the server cannot be inspected. This catches an
            // accidental/stale/wrong peer, but a basename is NOT a signature:
            // malicious software running as the same Windows user can spoof it.
            if verify_discord_peer(raw).is_err() {
                untrusted = true;
                continue;
            }
            return Ok(Self {
                handle,
                cancellation,
            });
        }
        Err(if untrusted {
            RpcError::UntrustedPeer
        } else {
            RpcError::Unavailable
        })
    }

    fn transfer(
        &self,
        buffer: *mut u8,
        count: usize,
        write: bool,
        timeout: Duration,
    ) -> Result<Option<usize>, RpcError> {
        if self.cancellation.load(Ordering::Acquire) {
            return Err(RpcError::Cancelled);
        }
        let mut operation = Operation::new(self.handle.0)?;
        let mut transferred = 0u32;
        // The buffer remains borrowed by read/write_all until this function
        // returns. Operation's Drop cancels AND joins pending I/O before either
        // the pinned allocation or that borrow can be released, including error
        // and unwinding paths. Box keeps OVERLAPPED at a stable address.
        let success = unsafe {
            if write {
                WriteFile(
                    self.handle.0,
                    buffer.cast_const(),
                    count as u32,
                    &mut transferred,
                    &mut *operation.overlapped,
                )
            } else {
                ReadFile(
                    self.handle.0,
                    buffer,
                    count as u32,
                    &mut transferred,
                    &mut *operation.overlapped,
                )
            }
        };
        if success != 0 {
            return Ok(Some(transferred as usize));
        }
        let code = unsafe { GetLastError() };
        if code == ERROR_MORE_DATA && !write && transferred > 0 {
            return Ok(Some(transferred as usize));
        }
        if code != ERROR_IO_PENDING {
            return Err(io_error(code));
        }
        operation.pending = true;
        let deadline = Instant::now() + timeout;
        loop {
            let cancelled = self.cancellation.load(Ordering::Acquire);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if cancelled || remaining.is_zero() {
                // CancelIoEx only REQUESTS cancellation. Always join before
                // releasing OVERLAPPED, its event, or the caller's buffer.
                unsafe {
                    CancelIoEx(self.handle.0, &*operation.overlapped);
                }
                let completion = operation.join();
                if cancelled {
                    return Err(RpcError::Cancelled);
                }
                return match completion {
                    Ok(n) => Ok(Some(n as usize)), // completion won the race
                    Err(ERROR_OPERATION_ABORTED) => Ok(None),
                    Err(code) => Err(io_error(code)),
                };
            }
            let milliseconds = remaining.min(Duration::from_millis(200)).as_millis().max(1) as u32;
            match unsafe { WaitForSingleObject(operation.event.0, milliseconds) } {
                WAIT_OBJECT_0 => {
                    return operation.join().map(|n| Some(n as usize)).map_err(io_error)
                }
                WAIT_TIMEOUT => continue,
                _ => {
                    let error = unsafe { GetLastError() };
                    // Operation::drop cancels and joins if the wait failed.
                    return Err(io_error(error));
                }
            }
        }
    }
}

impl Wire for Pipe {
    fn read(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<Option<usize>, RpcError> {
        if buffer.is_empty() {
            return Ok(Some(0));
        }
        self.transfer(
            buffer.as_mut_ptr(),
            buffer.len().min(u32::MAX as usize),
            false,
            timeout,
        )
    }

    fn write_all(&mut self, buffer: &[u8], timeout: Duration) -> Result<(), RpcError> {
        let deadline = Instant::now() + timeout;
        let mut written = 0;
        while written < buffer.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RpcError::Timeout);
            }
            let count = (buffer.len() - written).min(u32::MAX as usize);
            match self.transfer(
                buffer[written..].as_ptr().cast_mut(),
                count,
                true,
                remaining,
            )? {
                Some(0) => return Err(RpcError::Disconnected),
                Some(n) => written += n,
                None => return Err(RpcError::Timeout),
            }
        }
        Ok(())
    }
}

/// The destructor is a safety backstop: OVERLAPPED and its event must never be
/// freed while the kernel may still access them. Ordinary paths also join first.
struct Operation {
    pipe: HANDLE,
    overlapped: Box<OVERLAPPED>,
    event: Handle,
    pending: bool,
}

impl Operation {
    fn new(pipe: HANDLE) -> Result<Self, RpcError> {
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(RpcError::Io(unsafe { GetLastError() }));
        }
        let mut overlapped: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
        overlapped.hEvent = event;
        Ok(Self {
            pipe,
            overlapped,
            event: Handle(event),
            pending: false,
        })
    }

    fn join(&mut self) -> Result<u32, u32> {
        loop {
            let mut bytes = 0;
            let success =
                unsafe { GetOverlappedResult(self.pipe, &*self.overlapped, &mut bytes, 1) };
            if success != 0 {
                self.pending = false;
                return Ok(bytes);
            }
            let code = unsafe { GetLastError() };
            // A private manual-reset event is only signalled by this operation.
            // Defensive retry keeps the allocation alive even for INCOMPLETE.
            if code == ERROR_IO_INCOMPLETE || code == ERROR_IO_PENDING {
                continue;
            }
            self.pending = false;
            if code == ERROR_MORE_DATA && bytes > 0 {
                return Ok(bytes);
            }
            return Err(code);
        }
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if self.pending {
            unsafe {
                CancelIoEx(self.pipe, &*self.overlapped);
            }
            let _ = self.join();
        }
    }
}

fn io_error(code: u32) -> RpcError {
    match code {
        ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED => RpcError::Disconnected,
        ERROR_OPERATION_ABORTED => RpcError::Cancelled,
        _ => RpcError::Io(code),
    }
}

fn verify_discord_peer(pipe: HANDLE) -> Result<(), RpcError> {
    let mut pid = 0;
    if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 || pid == 0 {
        return Err(RpcError::UntrustedPeer);
    }
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(RpcError::UntrustedPeer);
    }
    let process = Handle(raw);
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err(RpcError::UntrustedPeer);
    }
    let path = String::from_utf16(&path[..length as usize]).map_err(|_| RpcError::UntrustedPeer)?;
    let basename = path.rsplit(['\\', '/']).next().unwrap_or("");
    if [
        "Discord.exe",
        "DiscordCanary.exe",
        "DiscordPTB.exe",
        "DiscordDevelopment.exe",
    ]
    .iter()
    .any(|name| basename.eq_ignore_ascii_case(name))
    {
        Ok(())
    } else {
        Err(RpcError::UntrustedPeer)
    }
}
