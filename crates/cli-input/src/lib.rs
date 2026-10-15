//! Bounded secret input shared by operator commands.
//!
//! A terminal gets hidden, Enter-terminated input. Redirected stdin is read once
//! to EOF and never falls back to a controlling terminal, including in CI.

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal,
};
use std::io::{self, IsTerminal, Read, Write};
use zeroize::Zeroizing;

const PASSWORD_MAX: usize = 1024;
const TOKEN_MAX: usize = 256;
const PASSWORD_PROMPT: &str = "请输入管理员密码（12–1024 字节，输入隐藏，回车结束）：";
const CONFIRM_PROMPT: &str = "请再次输入管理员密码（输入隐藏，回车结束）：";
const TOKEN_PROMPT: &str = "请输入一次性注册令牌（输入隐藏，回车结束）：";

#[derive(Clone, Copy)]
struct SecretSpec {
    prompt: &'static str,
    min: usize,
    max: usize,
    length_error: &'static str,
}

const PASSWORD: SecretSpec = SecretSpec {
    prompt: PASSWORD_PROMPT,
    min: 12,
    max: PASSWORD_MAX,
    length_error: "密码长度必须为 12–1024 字节",
};
const TOKEN: SecretSpec = SecretSpec {
    prompt: TOKEN_PROMPT,
    min: 1,
    max: TOKEN_MAX,
    length_error: "注册令牌长度必须为 1–256 字节",
};

/// Read a new admin password, confirming it only when stdin is a terminal.
/// Leading/trailing spaces are significant. No secret is printed or logged.
pub fn read_admin_password() -> io::Result<String> {
    read_secret(PASSWORD, Some(CONFIRM_PROMPT))
}

/// Read an enrollment token once, without a confirmation prompt.
pub fn read_enrollment_token() -> io::Result<String> {
    read_secret(TOKEN, None)
}

fn read_secret(spec: SecretSpec, confirmation: Option<&'static str>) -> io::Result<String> {
    // This gate is deliberately before every terminal API: a process may have a
    // controlling terminal even though its stdin is a pipe or redirected file.
    let stdin = io::stdin();
    if !stdin.is_terminal() {
        return read_redirected(stdin.lock(), spec);
    }
    // Keep echo disabled across both password prompts, including fast pastes.
    let mut mode = RawModeGuard::new()?;
    let result = (|| {
        let mut password = Zeroizing::new(read_hidden_line(spec)?);
        if let Some(prompt) = confirmation {
            let confirmation_spec = SecretSpec { prompt, ..spec };
            let confirmation = Zeroizing::new(read_hidden_line(confirmation_spec)?);
            if *password != *confirmation {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "两次输入的密码不一致；未创建管理员",
                ));
            }
        }
        Ok(std::mem::take(&mut *password))
    })();
    mode.restore()?;
    result
}

fn read_redirected(reader: impl Read, spec: SecretSpec) -> io::Result<String> {
    // Allow one optional LF/CRLF plus one extra byte to detect overflow. Never
    // truncate an oversized input into a different, apparently valid secret.
    let mut bytes = Zeroizing::new(Vec::new());
    reader.take((spec.max + 3) as u64).read_to_end(&mut bytes)?;
    if bytes.ends_with(b"\n") {
        bytes.pop();
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
    }
    validate_length(bytes.len(), spec)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "输入必须是有效的 UTF-8"))?;
    Ok(text.to_owned())
}

fn validate_length(length: usize, spec: SecretSpec) -> io::Result<()> {
    if !(spec.min..=spec.max).contains(&length) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            spec.length_error,
        ));
    }
    Ok(())
}

/// Restores the original mode on all normal/error/cancellation exits and unwind.
/// Ctrl-C is read as a key in raw mode, so restoration precedes returning it.
struct RawModeGuard {
    active: bool,
    restore: bool,
    #[cfg(windows)]
    original_mode: u32,
}

impl RawModeGuard {
    fn new() -> io::Result<Self> {
        #[cfg(windows)]
        let original_mode = windows_console_mode()?;
        let restore = !terminal::is_raw_mode_enabled()?;
        if restore {
            terminal::enable_raw_mode()?;
        }
        Ok(Self {
            active: true,
            restore,
            #[cfg(windows)]
            original_mode,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        // A fast paste can include a confirmation after an invalid first line,
        // or text after Ctrl-C. Discard it while echo is still disabled so it
        // cannot leak into the invoking shell. Always restore even if flush fails.
        let flushed = flush_pending_input();
        if self.restore {
            #[cfg(not(windows))]
            terminal::disable_raw_mode()?;
            #[cfg(windows)]
            restore_windows_console_mode(self.original_mode)?;
        }
        self.active = false;
        flushed
    }
}

#[cfg(windows)]
fn windows_console_mode() -> io::Result<u32> {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};
    let mut mode = 0;
    // Borrow the process's input handle. The caller has verified it is a TTY.
    if unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(mode)
}

#[cfg(windows)]
fn restore_windows_console_mode(mode: u32) -> io::Result<()> {
    use windows_sys::Win32::System::Console::{GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE};
    // Restore the exact original mask; crossterm's Windows disable_raw_mode
    // unconditionally enables echo, which is wrong if echo was already off.
    if unsafe { SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn flush_pending_input() -> io::Result<()> {
    use std::time::Duration;
    flush_os_input()?;
    // The OS flush cannot remove events crossterm has already buffered. A
    // nonzero, short poll is needed for its level-triggered Unix event source.
    // Bound cleanup so a continuously writing peer cannot delay restoration.
    let drained: io::Result<()> = (|| {
        for _ in 0..4096 {
            if !event::poll(Duration::from_millis(1))? {
                break;
            }
            let _ = event::read()?;
        }
        Ok(())
    })();
    // Also discard bytes that arrived during event cleanup, including a tail
    // beyond the bounded drain, immediately before restoring the saved mode.
    let flushed = flush_os_input();
    drained?;
    flushed
}

fn flush_os_input() -> io::Result<()> {
    #[cfg(unix)]
    {
        use nix::sys::termios::{tcflush, FlushArg};
        tcflush(io::stdin(), FlushArg::TCIFLUSH).map_err(io::Error::from)?;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            FlushConsoleInputBuffer, GetStdHandle, STD_INPUT_HANDLE,
        };
        if unsafe { FlushConsoleInputBuffer(GetStdHandle(STD_INPUT_HANDLE)) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn read_hidden_line(spec: SecretSpec) -> io::Result<String> {
    // Disable echo before showing a prompt. A fast paste must not race the
    // terminal configuration. Do not enable bracketed-paste buffering.
    let mut stderr = io::stderr().lock();
    let result = (|| {
        stderr.write_all(spec.prompt.as_bytes())?;
        stderr.flush()?;
        read_events(event::read, spec)
    })();
    let newline = stderr.write_all(b"\r\n").and_then(|()| stderr.flush());
    newline?;
    result
}

fn read_events(
    mut next_event: impl FnMut() -> io::Result<Event>,
    spec: SecretSpec,
) -> io::Result<String> {
    let mut secret = Zeroizing::new(String::new());
    let mut too_long = false;
    loop {
        let Event::Key(key) = next_event()? else {
            continue;
        };
        // Windows emits both press and release events. Never append twice.
        if key.kind == KeyEventKind::Release {
            continue;
        }
        // Windows represents AltGr-produced characters as Ctrl+Alt. They are
        // printable input, not cancellation/editing shortcuts.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            match key.code {
                KeyCode::Char('c' | 'C') => {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "输入已取消"));
                }
                KeyCode::Char('d' | 'D' | 'z' | 'Z') => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "输入已结束；未提交",
                    ));
                }
                KeyCode::Char('u' | 'U') => {
                    secret.clear();
                    too_long = false;
                    continue;
                }
                _ => continue,
            }
        }
        match key.code {
            KeyCode::Enter => {
                if too_long {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        spec.length_error,
                    ));
                }
                validate_length(secret.len(), spec)?;
                return Ok(std::mem::take(&mut *secret));
            }
            KeyCode::Esc => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "输入已取消"));
            }
            KeyCode::Backspace => {
                secret.pop();
            }
            KeyCode::Char(c) if !c.is_control() => {
                if !too_long && secret.len() + c.len_utf8() <= spec.max {
                    secret.push(c);
                } else {
                    // Keep draining hidden input through Enter after overflow.
                    // This bounds storage without leaving pasted secret bytes
                    // to be echoed or interpreted by the invoking shell.
                    too_long = true;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
