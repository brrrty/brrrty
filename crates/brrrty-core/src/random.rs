//! Random numbers for the filters a config declares random
//! (`config.add_random_filter`), and which page drew them.
//!
//! A filter that calls JavaScript's `Math.random` makes 11ty write a
//! different page on every build, so there is no single output of 11ty's
//! to match: any result the filter can give is one 11ty could have
//! written. Such a filter runs, and the page it renders on gets a note.
//!
//! Each render of a page draws from a stream of its own, so the draws
//! don't depend on which pages other threads render meanwhile. Unseeded,
//! every stream starts from this process's own random value, as
//! `Math.random` does in each 11ty run. `BRRRTY_RANDOM_SEED` (Brrrty's own;
//! 11ty has no seed) starts each stream from the seed and the page instead,
//! so every build with the same seed writes the same pages.

use std::cell::RefCell;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The variable that seeds the draws.
pub const SEED_VAR: &str = "BRRRTY_RANDOM_SEED";

/// The page rendering on this thread, and its draws so far.
struct Draws {
    shown: PathBuf,
    /// The render's own name (what it renders, and of which page), which
    /// seeds its stream.
    label: String,
    /// Not drawn from yet: seeded at the first draw.
    state: Option<u64>,
}

thread_local! {
    static PAGE: RefCell<Option<Draws>> = const { RefCell::new(None) };
}

/// While alive, random draws on this thread are the render `label` of the
/// page from `shown`, and start a fresh stream.
pub(crate) struct PageGuard(Option<Draws>);

impl PageGuard {
    pub(crate) fn enter(shown: &Path, label: String) -> PageGuard {
        let draws = Draws {
            shown: shown.to_path_buf(),
            label,
            state: None,
        };
        PageGuard(PAGE.with(|p| p.borrow_mut().replace(draws)))
    }
}

impl Drop for PageGuard {
    fn drop(&mut self) {
        let before = self.0.take();
        PAGE.with(|p| *p.borrow_mut() = before);
    }
}

/// The source of the page rendering on this thread (as the report shows
/// it), if any.
pub fn current_page() -> Option<PathBuf> {
    PAGE.with(|p| p.borrow().as_ref().map(|d| d.shown.clone()))
}

/// A number in `[0, 1)`, as `Math.random()` gives.
pub fn next_f64() -> f64 {
    let bits = PAGE.with(|p| match p.borrow_mut().as_mut() {
        Some(d) => {
            let state = d.state.get_or_insert_with(|| stream_start(&d.label));
            splitmix(state)
        }
        None => {
            // Outside any page: one stream for the process.
            static LOOSE: LazyLock<std::sync::Mutex<u64>> =
                LazyLock::new(|| std::sync::Mutex::new(stream_start("")));
            let mut s = LOOSE.lock().unwrap_or_else(|e| e.into_inner());
            splitmix(&mut s)
        }
    });
    // The top 53 bits, as a double's mantissa holds.
    (bits >> 11) as f64 / (1u64 << 53) as f64
}

/// Where a stream starts: from the seed and `label`, or, unseeded, from
/// this process's random value and a count of the streams started.
fn stream_start(label: &str) -> u64 {
    match std::env::var(SEED_VAR) {
        Ok(seed) if !seed.is_empty() => {
            let mut h = fnv(seed.as_bytes());
            h ^= fnv(label.as_bytes()).rotate_left(17);
            h
        }
        _ => {
            static PROCESS: LazyLock<u64> = LazyLock::new(|| {
                let mut h = std::collections::hash_map::RandomState::new().build_hasher();
                h.write_u128(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or_default(),
                );
                h.finish()
            });
            static COUNT: AtomicU64 = AtomicU64::new(0);
            let n = COUNT.fetch_add(1, Ordering::Relaxed);
            *PROCESS ^ n.wrapping_mul(0xA076_1D64_78BD_642F)
        }
    }
}

/// FNV-1a: the same on every platform and every Rust.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// SplitMix64's next value.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_are_in_zero_to_one() {
        let _g = PageGuard::enter(Path::new("a.njk"), "a".into());
        for _ in 0..10_000 {
            let x = next_f64();
            assert!((0.0..1.0).contains(&x), "{x}");
        }
    }

    #[test]
    fn a_page_scope_restores_the_one_before() {
        assert_eq!(current_page(), None);
        let outer = PageGuard::enter(Path::new("a.njk"), "a".into());
        {
            let _inner = PageGuard::enter(Path::new("b.njk"), "b".into());
            assert_eq!(current_page(), Some(PathBuf::from("b.njk")));
        }
        assert_eq!(current_page(), Some(PathBuf::from("a.njk")));
        drop(outer);
        assert_eq!(current_page(), None);
    }

    #[test]
    fn splitmix_matches_its_reference() {
        // SplitMix64 from 0: the reference implementation's first values.
        let mut s = 0;
        assert_eq!(splitmix(&mut s), 0xE220_A839_7B1D_CDAF);
        assert_eq!(splitmix(&mut s), 0x6E78_9E6A_A1B9_65F4);
    }
}
