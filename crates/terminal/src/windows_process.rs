use std::{
    io,
    os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle},
};
use windows_sys::Win32::System::Threading::TerminateProcess;

pub fn clone_handle(child: &dyn portable_pty::Child) -> io::Result<OwnedHandle> {
    let raw = child
        .as_raw_handle()
        .ok_or_else(|| io::Error::other("PTY child has no process handle"))?;
    // The child still owns this valid handle. Duplicate it now so signaling is
    // independent of the wait thread and can never target a recycled process ID.
    unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()
}

pub fn terminate(process: &OwnedHandle) -> io::Result<()> {
    // portable-pty 0.9.0's WinChildKiller inverts this BOOL return value. Use the
    // owned process handle directly: nonzero means the asynchronous request was
    // accepted, zero means GetLastError is meaningful. Waiting remains separate.
    // https://learn.microsoft.com/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
    if unsafe { TerminateProcess(process.as_raw_handle(), 1) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[test]
    fn a_handle_without_termination_rights_returns_an_error() {
        // Deliberately request only query rights. This handle cannot terminate
        // the test process, and proves zero is not incorrectly called accepted.
        let raw =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, GetCurrentProcessId()) };
        assert!(!raw.is_null());
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        assert!(terminate(&handle).is_err());
    }
}
