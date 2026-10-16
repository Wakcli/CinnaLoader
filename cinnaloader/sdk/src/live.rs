//! The host's view of the running client.
//!
//! The host samples what it can see and publishes it as `key<TAB>value` lines through the
//! `observe` slot. Today the host's own baseline is what fills it — a frame counter, its
//! tick count, the mods it loaded and the phase it is in — and the reads that come from a
//! running client are a later lane. A host outside a client, or one with nothing to report,
//! answers empty, and every accessor here returns `None` or the empty string rather than a
//! guess.

use std::ffi::{c_char, CStr};

/// The host's observation of the running client, as `key<TAB>value` lines. Empty when the
/// host has no client to observe.
#[must_use]
pub fn observation() -> String {
    read().unwrap_or_default()
}

/// The value for `key`, the last one when a key repeats, or `None` when there is no host or
/// no such key.
#[must_use]
pub fn value(key: &str) -> Option<String> {
    observation()
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.to_owned())
        .next_back()
}

/// `key`'s value read as an unsigned integer, or `None`.
#[must_use]
pub fn u64(key: &str) -> Option<u64> {
    value(key)?.trim().parse().ok()
}

/// `key`'s value read as a float, or `None`.
#[must_use]
pub fn f32(key: &str) -> Option<f32> {
    value(key)?.trim().parse().ok()
}

/// `key`'s value as text, or `None` when the key is absent. An empty value is still a value.
#[must_use]
pub fn text(key: &str) -> Option<String> {
    value(key)
}

/// Reads the host's observation, from a stack buffer first and a heap buffer when the
/// observation is longer than the stack holds.
fn read() -> Option<String> {
    let observe = *crate::OBSERVE.get()?;
    let mut stack = [0 as c_char; 512];
    // SAFETY: `observe` writes at most `stack.len()` bytes, NUL-terminated.
    if unsafe { observe(stack.as_mut_ptr(), stack.len()) } > 0 {
        // SAFETY: the host wrote a NUL-terminated blob that fits.
        return Some(
            unsafe { CStr::from_ptr(stack.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        );
    }
    // Either there is nothing to report, or the observation is longer than the stack buffer:
    // ask once more into a heap buffer big enough for any honest observation.
    crate::read_blob(observe, 64 * 1024)
}
