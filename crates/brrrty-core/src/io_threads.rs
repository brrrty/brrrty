//! The threads that create output folders and files.
//!
//! On APFS, many threads creating files and folders under the same parents
//! fight over the folder locks: writing the pages of a 1,200-page site into a
//! clean folder took 200 ms and 1.5 s of system time on 10 threads, and 105 ms
//! and 0.4 s on 4. Cloning 1,000 passthrough files behaved the same way. So
//! on macOS output is written by a small pool of its own; elsewhere it uses
//! the render threads, as before.

use std::sync::OnceLock;

/// How many threads write output on macOS. Four was the fastest of 1 to 10 on
/// an M4 (4 performance cores), for both pages and passthrough files.
#[cfg(target_os = "macos")]
const MACOS_WRITERS: usize = 4;

/// The pool output is written on, or `None` to use the current pool.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let writers = writers(rayon::current_num_threads())?;
        rayon::ThreadPoolBuilder::new()
            .num_threads(writers)
            .thread_name(|i| format!("brrrty-write-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// How many writer threads to use given `available` threads, or `None` when
/// writing on the current pool is just as good.
fn writers(available: usize) -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        (available > MACOS_WRITERS).then_some(MACOS_WRITERS)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = available;
        None
    }
}

/// Run `f`, whose parallel iterators write output files, on the writer threads.
pub(crate) fn install<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    match pool() {
        Some(pool) => pool.install(f),
        None => f(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn macos_caps_writers_only_when_there_are_more_threads() {
        assert_eq!(writers(10), Some(MACOS_WRITERS));
        assert_eq!(writers(MACOS_WRITERS), None);
        assert_eq!(writers(1), None);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn other_systems_keep_the_current_pool() {
        assert_eq!(writers(64), None);
    }

    #[test]
    fn install_runs_parallel_work_and_keeps_order() {
        let out: Vec<usize> = install(|| (0..1000).into_par_iter().map(|i| i * 2).collect());
        assert_eq!(out, (0..1000).map(|i| i * 2).collect::<Vec<_>>());
    }
}
