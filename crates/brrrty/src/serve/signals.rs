//! Ctrl-C and SIGTERM for `brrrty serve`, and for `brrrty doctor --verify`, which only
//! cleans up and exits (see [`Interrupts::on_signal`]).
//!
//! The first signal asks the session to stop: it finishes a build already running, leaves the
//! last good build in the output folder, closes the server and removes the staging folder. A
//! second signal ends the process at once, as the signal's default action would.
//!
//! On SIGINT the process then exits 0, as `eleventy --serve` does on Ctrl-C. SIGTERM ends
//! `eleventy --serve` through the signal's default action; [`Interrupts::finish`] does the same
//! once the clean stop is done, so a supervisor sees the same status.

use std::io;
use std::sync::{Arc, Mutex};

use super::StopHandle;

/// Run on the first signal, on the signal thread.
type OnSignal = Box<dyn Fn(i32) + Send>;

#[derive(Default)]
struct State {
    stop: Option<StopHandle>,
    on_signal: Option<OnSignal>,
    /// The first signal received.
    signal: Option<i32>,
}

/// Catches Ctrl-C and SIGTERM for as long as the process runs.
#[derive(Clone)]
pub struct Interrupts(Arc<Mutex<State>>);

impl Interrupts {
    /// Start catching the signals. On platforms other than Unix this catches nothing and the
    /// signals keep their default action; the output folder is current after every good
    /// build, so it is left complete either way.
    pub fn install() -> io::Result<Interrupts> {
        let state = Arc::new(Mutex::new(State::default()));
        #[cfg(unix)]
        {
            use signal_hook::consts::{SIGINT, SIGTERM};
            use signal_hook::iterator::Signals;
            let mut signals = Signals::new([SIGINT, SIGTERM])?;
            let shared = Arc::clone(&state);
            std::thread::Builder::new()
                .name("brrrty-signals".into())
                .spawn(move || {
                    for sig in signals.forever() {
                        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                        if s.signal.is_some() {
                            // The second one: don't wait for the clean stop.
                            let _ = signal_hook::low_level::emulate_default_handler(sig);
                            std::process::exit(1);
                        }
                        s.signal = Some(sig);
                        if let Some(h) = &s.stop {
                            h.stop();
                        }
                        if let Some(f) = &s.on_signal {
                            f(sig);
                        }
                    }
                })?;
        }
        Ok(Interrupts(state))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stop this session on the next signal, or at once if one came already.
    pub fn stop_with(&self, handle: StopHandle) {
        let mut s = self.lock();
        if s.signal.is_some() {
            handle.stop();
        }
        s.stop = Some(handle);
    }

    /// Run `f` with the signal on the first one, or at once if one came already. It runs on
    /// the signal thread while the other threads go on, so it may end the process.
    pub fn on_signal(&self, f: impl Fn(i32) + Send + 'static) {
        let mut s = self.lock();
        if let Some(sig) = s.signal {
            f(sig);
        }
        s.on_signal = Some(Box::new(f));
    }

    /// Whether a signal has come.
    pub fn received(&self) -> bool {
        self.lock().signal.is_some()
    }

    /// Call once the clean stop is done. After SIGTERM this ends the process the way the
    /// signal's default action does; otherwise it returns.
    pub fn finish(&self) {
        #[cfg(unix)]
        if self.lock().signal == Some(signal_hook::consts::SIGTERM) {
            let _ = signal_hook::low_level::emulate_default_handler(signal_hook::consts::SIGTERM);
        }
    }
}

/// The variable the npm launcher sets to its own process id. The launcher
/// passes Ctrl-C and other signals on, but nothing can pass on a SIGKILL, and
/// it starts the binary in a session of its own, so the terminal can't reach
/// it either: without this, killing the launcher would leave `brrrty serve`
/// running, holding its port.
pub const LAUNCHER_PID_VAR: &str = "BRRRTY_LAUNCHER_PID";

/// When started by the npm launcher, stop the way SIGTERM does (cleanly, then
/// ending by the signal) once the launcher is gone. Call after
/// [`Interrupts::install`]. Does nothing otherwise.
pub fn stop_with_launcher() {
    #[cfg(unix)]
    {
        let Some(launcher) = std::env::var(LAUNCHER_PID_VAR)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
        else {
            return;
        };
        let gone = move || std::os::unix::process::parent_id() != launcher;
        let stop = || {
            let _ = signal_hook::low_level::raise(signal_hook::consts::SIGTERM);
        };
        // Look every half second: once the launcher is gone, this process is
        // handed to another parent. (Linux's PR_SET_PDEATHSIG would be quicker,
        // but it fires again for each of the launcher's threads as they exit,
        // and a second SIGTERM means "quit now", skipping the clean stop.)
        let _ = std::thread::Builder::new()
            .name("brrrty-launcher-watch".into())
            .spawn(move || {
                while !gone() {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
                stop();
            });
    }
}
