// SPDX-License-Identifier: GPL-3.0-or-later
//! Restore terminal state on every exit, including incomplete setup and panic.

use std::{
    cell::Cell,
    io::{self, Write},
    panic,
    sync::Once,
};

use crossterm::{
    cursor::{Hide, Show},
    event::{DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

trait Lifecycle {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn enter_screen(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
    fn leave_screen(&mut self) -> io::Result<()>;
}

struct Session<L: Lifecycle> {
    lifecycle: L,
    raw_attempted: bool,
    screen_attempted: bool,
}

impl<L: Lifecycle> Session<L> {
    fn start(lifecycle: L) -> Result<Self, String> {
        let mut session = Self {
            lifecycle,
            raw_attempted: true,
            screen_attempted: false,
        };
        session
            .lifecycle
            .enable_raw()
            .map_err(|error| format!("could not enable terminal raw mode: {error}"))?;
        // A write or flush can fail after only some setup commands reached the
        // terminal. Mark the attempt first so Drop still undoes those commands.
        session.screen_attempted = true;
        session
            .lifecycle
            .enter_screen()
            .map_err(|error| format!("could not enter the alternate terminal screen: {error}"))?;
        Ok(session)
    }

    fn restore(&mut self) {
        if std::mem::take(&mut self.raw_attempted) {
            let _ = self.lifecycle.disable_raw();
        }
        if std::mem::take(&mut self.screen_attempted) {
            let _ = self.lifecycle.leave_screen();
        }
    }
}

impl<L: Lifecycle> Drop for Session<L> {
    fn drop(&mut self) {
        self.restore();
    }
}

fn with_lifecycle<L: Lifecycle, T>(
    lifecycle: L,
    work: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _session = Session::start(lifecycle)?;
    work()
}

pub(super) fn run<T>(work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    install_panic_hook();
    with_lifecycle(NativeLifecycle(io::stdout()), work)
}

struct NativeLifecycle(io::Stdout);

impl Lifecycle for NativeLifecycle {
    fn enable_raw(&mut self) -> io::Result<()> {
        TERMINAL_ACTIVE.with(|active| active.set(true));
        enable_raw_mode()
    }

    fn enter_screen(&mut self) -> io::Result<()> {
        enter_screen(&mut self.0)
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        TERMINAL_ACTIVE.with(|active| active.set(false));
        disable_raw_mode()
    }

    fn leave_screen(&mut self) -> io::Result<()> {
        leave_screen(&mut self.0)
    }
}

fn enter_screen(output: &mut impl Write) -> io::Result<()> {
    execute!(
        output,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange,
        Hide
    )
}

fn leave_screen(output: &mut impl Write) -> io::Result<()> {
    // Attempt every reset even if an earlier write fails. In particular, a
    // mouse/focus reset must not prevent showing the cursor or leaving the screen.
    let focus = execute!(output, DisableFocusChange);
    let mouse = execute!(output, DisableMouseCapture);
    let cursor = execute!(output, Show);
    let screen = execute!(output, LeaveAlternateScreen);
    focus.and(mouse).and(cursor).and(screen)
}

thread_local! {
    static TERMINAL_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

fn restore_before_report(active: &Cell<bool>, restore: impl FnOnce(), report: impl FnOnce()) {
    if active.replace(false) {
        restore();
    }
    report();
}

fn install_panic_hook() {
    static INSTALL: Once = Once::new();
    // Install one chained dispatcher for the process, rather than swapping
    // hooks around each session. Other threads have no active terminal session.
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            TERMINAL_ACTIVE.with(|active| {
                restore_before_report(
                    active,
                    || {
                        let mut lifecycle = NativeLifecycle(io::stdout());
                        let _ = lifecycle.disable_raw();
                        let _ = lifecycle.leave_screen();
                    },
                    || previous(info),
                );
            });
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        panic::{AssertUnwindSafe, catch_unwind},
        rc::Rc,
    };

    #[derive(Default)]
    struct State {
        events: Vec<&'static str>,
        raw: bool,
        screen: bool,
    }

    struct FakeLifecycle {
        state: Rc<RefCell<State>>,
        fail: Option<&'static str>,
    }

    impl FakeLifecycle {
        fn operation(&mut self, name: &'static str) -> io::Result<()> {
            self.state.borrow_mut().events.push(name);
            if self.fail == Some(name) {
                Err(io::Error::other(name))
            } else {
                Ok(())
            }
        }
    }

    impl Lifecycle for FakeLifecycle {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.state.borrow_mut().raw = true;
            self.operation("enable raw")
        }

        fn enter_screen(&mut self) -> io::Result<()> {
            self.state.borrow_mut().screen = true;
            self.operation("enter screen")
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.state.borrow_mut().raw = false;
            self.operation("disable raw")
        }

        fn leave_screen(&mut self) -> io::Result<()> {
            self.state.borrow_mut().screen = false;
            self.operation("leave screen")
        }
    }

    fn fake(fail: Option<&'static str>) -> (FakeLifecycle, Rc<RefCell<State>>) {
        let state = Rc::new(RefCell::new(State::default()));
        (
            FakeLifecycle {
                state: Rc::clone(&state),
                fail,
            },
            state,
        )
    }

    fn assert_restored(state: &RefCell<State>) {
        let state = state.borrow();
        assert!(!state.raw, "raw mode remains active");
        assert!(!state.screen, "alternate screen remains active");
    }

    #[test]
    fn normal_return_restores_after_the_session_body() {
        let (lifecycle, state) = fake(None);
        let result = with_lifecycle(lifecycle, || {
            assert!(state.borrow().raw);
            assert!(state.borrow().screen);
            state.borrow_mut().events.push("work");
            Ok(7)
        });
        assert_eq!(result, Ok(7));
        assert_restored(&state);
        assert_eq!(
            state.borrow().events,
            [
                "enable raw",
                "enter screen",
                "work",
                "disable raw",
                "leave screen"
            ]
        );
    }

    #[test]
    fn partial_raw_setup_is_restored_without_entering_a_screen() {
        let (lifecycle, state) = fake(Some("enable raw"));
        let result = with_lifecycle(lifecycle, || -> Result<(), String> {
            panic!("work must not run when setup fails")
        });
        assert!(
            result
                .unwrap_err()
                .contains("could not enable terminal raw mode")
        );
        assert_restored(&state);
        assert_eq!(state.borrow().events, ["enable raw", "disable raw"]);
    }

    #[test]
    fn partial_screen_setup_restores_both_terminal_states() {
        let (lifecycle, state) = fake(Some("enter screen"));
        let result = with_lifecycle(lifecycle, || -> Result<(), String> {
            panic!("work must not run when setup fails")
        });
        assert!(
            result
                .unwrap_err()
                .contains("could not enter the alternate terminal screen")
        );
        assert_restored(&state);
        assert_eq!(
            state.borrow().events,
            ["enable raw", "enter screen", "disable raw", "leave screen"]
        );
    }

    #[test]
    fn early_error_preserves_the_original_error_and_restores_the_session() {
        let (lifecycle, state) = fake(None);
        let result = with_lifecycle(lifecycle, || Err::<(), _>("drawing failed".into()));
        assert_eq!(result, Err("drawing failed".into()));
        assert_restored(&state);
    }

    #[test]
    fn cleanup_error_does_not_skip_the_remaining_restoration_or_mask_the_result() {
        let (lifecycle, state) = fake(Some("disable raw"));
        let result = with_lifecycle(lifecycle, || Ok(1));
        assert_eq!(result, Ok(1));
        assert_restored(&state);
        assert_eq!(
            state.borrow().events,
            ["enable raw", "enter screen", "disable raw", "leave screen"]
        );
    }

    #[test]
    fn unwind_restores_the_session_without_touching_the_real_terminal() {
        let (lifecycle, state) = fake(None);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _ = with_lifecycle(lifecycle, || -> Result<(), String> {
                panic!("injected render panic");
            });
        }));
        assert!(result.is_err());
        assert_restored(&state);
        assert_eq!(
            state.borrow().events,
            ["enable raw", "enter screen", "disable raw", "leave screen"]
        );
    }

    #[test]
    fn repeated_restoration_is_idempotent() {
        let (lifecycle, state) = fake(None);
        let mut session = Session::start(lifecycle).unwrap();
        session.restore();
        session.restore();
        drop(session);
        assert_restored(&state);
        assert_eq!(
            state.borrow().events,
            ["enable raw", "enter screen", "disable raw", "leave screen"]
        );
    }

    #[test]
    fn panic_diagnostics_follow_restoration_and_clear_the_active_flag() {
        let active = Cell::new(true);
        let events = RefCell::new(Vec::new());
        restore_before_report(
            &active,
            || events.borrow_mut().push("restore"),
            || {
                assert!(!active.get());
                events.borrow_mut().push("report");
            },
        );
        assert_eq!(*events.borrow(), ["restore", "report"]);
        restore_before_report(
            &active,
            || panic!("an inactive session must not restore a terminal"),
            || events.borrow_mut().push("other report"),
        );
        assert_eq!(*events.borrow(), ["restore", "report", "other report"]);
    }

    #[test]
    fn screen_commands_reset_every_enabled_mode_and_show_the_cursor() {
        let mut output = Vec::new();
        enter_screen(&mut output).unwrap();
        assert!(output.ends_with(b"\x1b[?25l"));
        output.clear();
        leave_screen(&mut output).unwrap();
        assert!(output.starts_with(b"\x1b[?1004l"));
        assert!(output.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
        assert!(output.ends_with(b"\x1b[?1049l"));
    }

    #[test]
    fn a_failed_focus_reset_still_shows_the_cursor_and_leaves_the_screen() {
        #[derive(Default)]
        struct FailOnce {
            failed: bool,
            bytes: Vec<u8>,
        }
        impl Write for FailOnce {
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                if !std::mem::replace(&mut self.failed, true) {
                    return Err(io::Error::other("injected first-write failure"));
                }
                self.bytes.extend_from_slice(buffer);
                Ok(buffer.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut output = FailOnce::default();
        assert!(leave_screen(&mut output).is_err());
        assert!(output.bytes.windows(6).any(|bytes| bytes == b"\x1b[?25h"));
        assert!(output.bytes.ends_with(b"\x1b[?1049l"));
    }
}
