use super::*;
use crossterm::event::KeyEvent;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::{
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

// All fixtures are synthetic. Diagnostics deliberately never print input or
// captured output, even on assertion failures.
const SYNTHETIC: &str = "  fixture-密码-🦀  ";
const OTHER: &str = "different-fixture-value";
const FIXTURE_NAME: &str = "tests::secret_input_fixture";
const DONE: &[u8] = b"SECRET_FIXTURE_OK";

fn redirected(bytes: &[u8]) -> io::Result<String> {
    read_redirected(bytes, PASSWORD)
}

#[test]
fn pipeline_preserves_spaces_unicode_and_strips_one_line_ending() {
    for ending in ["", "\n", "\r\n"] {
        let result = redirected(format!("{SYNTHETIC}{ending}").as_bytes()).unwrap();
        assert!(result == SYNTHETIC);
    }
    assert!(redirected(format!("{SYNTHETIC}\n\n").as_bytes()).unwrap() == format!("{SYNTHETIC}\n"));
    assert!(redirected(format!("{SYNTHETIC}\r").as_bytes()).unwrap() == format!("{SYNTHETIC}\r"));
    assert!(
        redirected(format!("{SYNTHETIC}\r\n\r\n").as_bytes()).unwrap()
            == format!("{SYNTHETIC}\r\n")
    );
}

#[test]
fn pipeline_rejects_empty_short_oversized_and_invalid_utf8() {
    for bytes in [vec![], vec![b'x'; 11], vec![b'x'; 1025], vec![0xff; 12]] {
        assert!(redirected(&bytes).is_err());
    }
    for ending in ["", "\n", "\r\n"] {
        assert_eq!(
            redirected(format!("{}{ending}", "x".repeat(1024)).as_bytes())
                .unwrap()
                .len(),
            1024
        );
        assert!(redirected(format!("{}{ending}", "x".repeat(1025)).as_bytes()).is_err());
    }
    // Count bytes, not characters; never split and accept a truncated UTF-8 value.
    assert_eq!(redirected("密".repeat(341).as_bytes()).unwrap().len(), 1023);
    assert!(redirected("密".repeat(342).as_bytes()).is_err());
}

#[test]
fn pipeline_read_has_a_hard_bound_and_token_has_its_own_limit() {
    let mut infinite = io::repeat(b'x');
    assert!(read_redirected(&mut infinite, PASSWORD).is_err());
    assert_eq!(
        read_redirected(vec![b'x'; 256].as_slice(), TOKEN)
            .unwrap()
            .len(),
        256
    );
    assert!(read_redirected(vec![b'x'; 257].as_slice(), TOKEN).is_err());
    assert!(read_redirected(&b""[..], TOKEN).is_err());
}

fn key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn events(mut input: impl Iterator<Item = Event>, spec: SecretSpec) -> io::Result<String> {
    read_events(
        || {
            input
                .next()
                .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))
        },
        spec,
    )
}

#[test]
fn terminal_preserves_unicode_spaces_and_backspaces_whole_characters() {
    let input = SYNTHETIC.chars().map(|c| key(KeyCode::Char(c))).chain([
        key(KeyCode::Char('密')),
        key(KeyCode::Backspace),
        key(KeyCode::Enter),
    ]);
    assert!(events(input, PASSWORD).unwrap() == SYNTHETIC);
}

#[test]
fn terminal_ignores_windows_release_events_and_navigation() {
    let input = SYNTHETIC
        .chars()
        .flat_map(|c| {
            [
                key(KeyCode::Char(c)),
                Event::Key(KeyEvent::new_with_kind(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                    KeyEventKind::Release,
                )),
            ]
        })
        .chain([key(KeyCode::Left), key(KeyCode::Enter)]);
    assert!(events(input, PASSWORD).unwrap() == SYNTHETIC);
}

#[test]
fn terminal_accepts_windows_altgr_printable_characters() {
    let input = [
        Event::Key(KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )),
        Event::Key(KeyEvent::new(
            KeyCode::Char('€'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        )),
        key(KeyCode::Enter),
    ];
    assert!(events(input.into_iter(), TOKEN).unwrap() == "@€");
}

#[test]
fn terminal_cancellation_and_partial_eof_are_errors() {
    for c in ['c', 'd', 'z'] {
        let input = SYNTHETIC
            .chars()
            .map(|c| key(KeyCode::Char(c)))
            .chain([Event::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            ))]);
        assert!(events(input, PASSWORD).is_err());
    }
    assert!(events(SYNTHETIC.chars().map(|c| key(KeyCode::Char(c))), PASSWORD).is_err());
    assert!(events([key(KeyCode::Esc)].into_iter(), TOKEN).is_err());
    assert!(events([key(KeyCode::Enter)].into_iter(), TOKEN).is_err());
}

#[test]
fn terminal_overflow_stays_bounded_and_drains_until_enter() {
    let mut count = 0;
    let result = read_events(
        || {
            count += 1;
            Ok(if count <= 100_000 {
                key(KeyCode::Char('密'))
            } else {
                key(KeyCode::Enter)
            })
        },
        PASSWORD,
    );
    assert!(result.is_err());
    assert_eq!(count, 100_001);
}

#[test]
fn terminal_ctrl_u_can_restart_after_overflow() {
    let input = std::iter::repeat_n(key(KeyCode::Char('x')), 1025)
        .chain([Event::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        ))])
        .chain(SYNTHETIC.chars().map(|c| key(KeyCode::Char(c))))
        .chain([key(KeyCode::Enter)]);
    assert!(events(input, PASSWORD).unwrap() == SYNTHETIC);
}

#[test]
fn terminal_read_error_never_returns_partial_secret() {
    let mut emitted = false;
    let result = read_events(
        || {
            if emitted {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            emitted = true;
            Ok(key(KeyCode::Char('x')))
        },
        TOKEN,
    );
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
}

fn fixture_command() -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", FIXTURE_NAME, "--nocapture"]);
    command
}

fn check_echo_restored(expected_echo: bool) {
    assert!(
        !terminal::is_raw_mode_enabled().unwrap(),
        "raw mode was not restored"
    );
    #[cfg(unix)]
    {
        use nix::sys::termios::{tcgetattr, LocalFlags};
        let settings = tcgetattr(io::stdin()).unwrap();
        assert_eq!(
            settings.local_flags.contains(LocalFlags::ECHO),
            expected_echo,
            "terminal echo was not restored"
        );
        assert!(
            settings.local_flags.contains(LocalFlags::ICANON),
            "canonical input was not restored"
        );
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{
            GetConsoleMode, GetStdHandle, ENABLE_ECHO_INPUT, ENABLE_LINE_INPUT, STD_INPUT_HANDLE,
        };
        let mut mode = 0;
        // The fixture is running in a real ConPTY; the handle is borrowed.
        assert_ne!(
            unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) },
            0
        );
        assert_eq!(
            mode & ENABLE_ECHO_INPUT != 0,
            expected_echo,
            "console echo was not restored"
        );
        assert_ne!(
            mode & ENABLE_LINE_INPUT,
            0,
            "console line input was not restored"
        );
    }
}

#[test]
#[ignore = "test-only child process invoked by native PTY and pipe tests"]
fn secret_input_fixture() {
    let mode = std::env::var("ERGENT_TEST_SECRET_MODE").expect("fixture requires test mode");
    if mode == "pipe-with-controlling-terminal" {
        assert!(io::stdin().is_terminal());
        let result = fixture_command()
            .env("ERGENT_TEST_SECRET_MODE", "password")
            .env("ERGENT_TEST_SECRET_RESULT", "success")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut child = result;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(SYNTHETIC.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "redirected child failed");
        assert!(
            output.stderr.is_empty(),
            "redirected input unexpectedly prompted"
        );
        assert!(output.stdout.windows(DONE.len()).any(|w| w == DONE));
    } else {
        let no_echo = std::env::var_os("ERGENT_TEST_NO_ECHO").is_some();
        #[cfg(unix)]
        let initial_mode = io::stdin().is_terminal().then(|| {
            use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};
            let mut mode = tcgetattr(io::stdin()).unwrap();
            if no_echo {
                mode.local_flags.remove(LocalFlags::ECHO);
                tcsetattr(io::stdin(), SetArg::TCSANOW, &mode).unwrap();
            }
            mode
        });
        #[cfg(windows)]
        let initial_mode = io::stdin().is_terminal().then(|| {
            use windows_sys::Win32::System::Console::ENABLE_ECHO_INPUT;
            let mut mode = windows_console_mode().unwrap();
            if no_echo {
                mode &= !ENABLE_ECHO_INPUT;
                restore_windows_console_mode(mode).unwrap();
            }
            mode
        });
        let result = if mode == "token" {
            read_enrollment_token()
        } else {
            read_admin_password()
        };
        if io::stdin().is_terminal() {
            check_echo_restored(!no_echo);
            #[cfg(unix)]
            assert!(
                initial_mode.unwrap() == nix::sys::termios::tcgetattr(io::stdin()).unwrap(),
                "terminal mode changed"
            );
            #[cfg(windows)]
            assert_eq!(
                initial_mode.unwrap(),
                windows_console_mode().unwrap(),
                "console mode changed"
            );
        }
        let expected = std::env::var("ERGENT_TEST_SECRET_RESULT").unwrap();
        match expected.as_str() {
            "success" => assert!(
                result.is_ok_and(|secret| secret == SYNTHETIC),
                "fixture returned an incorrect value"
            ),
            "mismatch" => assert!(result.is_err_and(|error| error.to_string().contains("不一致"))),
            "interrupted" => {
                assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::Interrupted))
            }
            "eof" => {
                assert!(result.is_err_and(|error| error.kind() == io::ErrorKind::UnexpectedEof))
            }
            "invalid" => assert!(result.is_err_and(|error| matches!(
                error.kind(),
                io::ErrorKind::InvalidData | io::ErrorKind::InvalidInput
            ))),
            _ => panic!("invalid test fixture mode"),
        }
        if std::env::var_os("ERGENT_TEST_CHECK_PENDING").is_some() {
            assert!(
                !event::poll(Duration::from_millis(2)).unwrap(),
                "crossterm retained pending input"
            );
            println!("AFTER_SECRET_READY");
            io::stdout().flush().unwrap();
            let mut line = String::new();
            io::stdin().read_line(&mut line).unwrap();
            assert!(
                line == "shell-check\n" || line == "shell-check\r\n",
                "pending secret bytes leaked to the next terminal reader"
            );
        }
    }
    println!("SECRET_FIXTURE_OK");
}

struct PtyFixture {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    receiver: mpsc::Receiver<Vec<u8>>,
    output: Vec<u8>,
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

fn fixture_failure_code(output: &[u8]) -> &'static str {
    // Never include raw PTY output in CI diagnostics. These fixed strings can
    // identify a failed invariant without disclosing even synthetic secrets.
    for code in [
        "raw mode was not restored",
        "terminal echo was not restored",
        "canonical input was not restored",
        "console echo was not restored",
        "console line input was not restored",
        "terminal mode changed",
        "console mode changed",
        "fixture returned an incorrect value",
        "crossterm retained pending input",
        "pending secret bytes leaked to the next terminal reader",
        "redirected child failed",
    ] {
        if output.windows(code.len()).any(|w| w == code.as_bytes()) {
            return code;
        }
    }
    "unclassified fixture failure"
}

impl PtyFixture {
    fn new(mode: &str, expected: &str) -> Self {
        Self::with_options(mode, expected, false, false)
    }

    fn with_options(mode: &str, expected: &str, no_echo: bool, check_pending: bool) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(std::env::current_exe().unwrap());
        command.args(["--ignored", "--exact", FIXTURE_NAME, "--nocapture"]);
        command.env("ERGENT_TEST_SECRET_MODE", mode);
        command.env("ERGENT_TEST_SECRET_RESULT", expected);
        if no_echo {
            command.env("ERGENT_TEST_NO_ECHO", "1");
        }
        if check_pending {
            command.env("ERGENT_TEST_CHECK_PENDING", "1");
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();
        let mut reader = pair.master.try_clone_reader().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || sender.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            writer,
            receiver,
            output: Vec::new(),
            _master: pair.master,
        }
    }

    fn until(&mut self, marker: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !self.output.windows(marker.len()).any(|w| w == marker) {
            let left = deadline.saturating_duration_since(Instant::now());
            let bytes = self.receiver.recv_timeout(left).unwrap_or_else(|_| {
                panic!("PTY fixture timed out or exited; captured output withheld (diagnostic: {}; confirmation prompt: {}, bytes: {})",
                    fixture_failure_code(&self.output), self.output.windows(CONFIRM_PROMPT.len()).any(|w| w == CONFIRM_PROMPT.as_bytes()), self.output.len());
            });
            self.output.extend(bytes);
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn finish(mut self) {
        self.until(DONE);
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "PTY fixture failed; captured output withheld"
                );
                break;
            }
            assert!(Instant::now() < deadline, "PTY fixture did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        while let Ok(bytes) = self.receiver.try_recv() {
            self.output.extend(bytes);
        }
        for forbidden in [SYNTHETIC.as_bytes(), OTHER.as_bytes()] {
            assert!(
                !self.output.windows(forbidden.len()).any(|w| w == forbidden),
                "secret was echoed"
            );
        }
    }
}

impl Drop for PtyFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn native_pty_password_requires_two_hidden_enter_terminated_entries() {
    let mut fixture = PtyFixture::new("password", "success");
    fixture.until(PASSWORD_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r").as_bytes());
    fixture.until(CONFIRM_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r").as_bytes());
    fixture.finish();
}

#[test]
fn native_pty_fast_paste_keeps_echo_hidden_between_password_entries() {
    let mut fixture = PtyFixture::new("password", "success");
    fixture.until(PASSWORD_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r{SYNTHETIC}\r").as_bytes());
    fixture.finish();
}

#[test]
fn native_pty_mismatch_restores_echo() {
    let mut fixture = PtyFixture::new("password", "mismatch");
    fixture.until(PASSWORD_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r{OTHER}\r").as_bytes());
    fixture.finish();
}

#[test]
fn native_pty_token_needs_one_hidden_entry() {
    let mut fixture = PtyFixture::new("token", "success");
    fixture.until(TOKEN_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r").as_bytes());
    fixture.finish();
}

#[test]
fn native_pty_ctrl_c_and_eof_cancel_both_password_prompts() {
    for (cancel, expected) in [(b'\x03', "interrupted"), (b'\x04', "eof"), (b'\x1a', "eof")] {
        for confirm in [false, true] {
            let mut fixture = PtyFixture::new("password", expected);
            fixture.until(PASSWORD_PROMPT.as_bytes());
            if confirm {
                fixture.send(format!("{SYNTHETIC}\r").as_bytes());
                fixture.until(CONFIRM_PROMPT.as_bytes());
            }
            fixture.send(&[cancel]);
            fixture.finish();
        }
    }
}

#[test]
fn native_pty_empty_and_oversized_inputs_fail_with_echo_restored() {
    for input in ["\r".to_owned(), format!("{}\r", "x".repeat(1025))] {
        let mut fixture = PtyFixture::new("password", "invalid");
        fixture.until(PASSWORD_PROMPT.as_bytes());
        fixture.send(input.as_bytes());
        fixture.finish();
    }
}

#[test]
fn redirected_input_ignores_an_available_controlling_terminal() {
    PtyFixture::new("pipe-with-controlling-terminal", "success").finish();
}

#[test]
fn native_pty_preserves_preexisting_disabled_echo_and_exact_terminal_mode() {
    let mut fixture = PtyFixture::with_options("token", "success", true, false);
    fixture.until(TOKEN_PROMPT.as_bytes());
    fixture.send(format!("{SYNTHETIC}\r").as_bytes());
    fixture.finish();
}

#[test]
fn native_pty_early_error_and_cancel_flush_pasted_input_before_restoring_echo() {
    for (prefix, expected) in [
        ("short\r", "invalid"),
        ("\x03", "interrupted"),
        ("\x04", "eof"),
    ] {
        let mut fixture = PtyFixture::with_options("password", expected, false, true);
        fixture.until(PASSWORD_PROMPT.as_bytes());
        fixture.send(format!("{prefix}{}\r", OTHER.repeat(120)).as_bytes());
        fixture.until(b"AFTER_SECRET_READY");
        fixture.send(b"shell-check\r");
        fixture.finish();
    }
}
