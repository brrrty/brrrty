//! Panic containment: run code that may panic and turn the panic into an error.
//!
//! The CLI installs a panic hook that prints a bug report and exits with 101. That hook runs
//! before any unwinding, so it would end the process before a `catch_unwind` could recover.
//! [`catch`] marks the current thread as contained while its closure runs, and the hook checks
//! [`is_active`] and stands down, leaving the panic to `catch_unwind`.
//!
//! The mark is per thread. Every containment wrapper runs its closure on the thread that calls
//! it (a Rayon worker during a parallel build), and none of the contained code starts parallel
//! work of its own, so a contained panic always happens on a marked thread. A panic anywhere
//! else still reaches the hook and exits with 101.

use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static LOCATION: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Whether the current thread is inside [`catch`].
pub fn is_active() -> bool {
    DEPTH.with(|d| d.get() > 0)
}

/// Record where a contained panic happened, for the message [`catch`] returns. Called by a
/// panic hook that stood down.
pub fn note_location(location: String) {
    LOCATION.with(|l| *l.borrow_mut() = Some(location));
}

/// Runs `f`, returning the panic's message (and, when a hook noted it, its location) if it
/// panicked.
pub fn catch<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    struct Mark;
    impl Drop for Mark {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get() - 1));
        }
    }
    DEPTH.with(|d| d.set(d.get() + 1));
    LOCATION.with(|l| l.borrow_mut().take());
    let mark = Mark;
    let result = catch_unwind(AssertUnwindSafe(f));
    drop(mark);
    result.map_err(|panic| {
        let why = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "it panicked".into());
        match LOCATION.with(|l| l.borrow_mut().take()) {
            Some(at) => format!("{why} (at {at})"),
            None => why,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_only_while_running() {
        assert!(!is_active());
        let inner = catch(|| {
            assert!(is_active());
            catch(is_active).unwrap()
        });
        assert_eq!(inner, Ok(true));
        assert!(!is_active());
    }

    #[test]
    fn a_panic_becomes_its_message_and_clears_the_mark() {
        let r: Result<(), String> = catch(|| panic!("boom {}", 1));
        assert_eq!(r, Err("boom 1".into()));
        assert!(!is_active());
    }
}
