//! Time access, per target. The one seam the core reads a clock through.
//!
//! `std::time::Instant::now()` and `SystemTime::now()` do not merely return an
//! error on `wasm32-unknown-unknown` — they reach an `unreachable` instruction,
//! which is a wasm **trap**, not a panic. `panic = "abort"` means `catch_unwind`
//! cannot contain it, and a trap poisons the module instance, so recovering
//! means reloading the whole worker. Verified against the bundled toolchain:
//! both calls produce `RuntimeError: unreachable`.
//!
//! That matters more than it looks, because the core reads the clock from
//! `open_db`, `import_csv_into`, `finalize_import_run_into`, `purge_old`,
//! `repair_fts_index`, `now_iso` and `window_day_hours` — so merely *opening a
//! database* would take the module down. The crate compiles for wasm either
//! way, which is exactly why this needed finding at runtime rather than by
//! reading the build output.
//!
//! Natively this is a straight re-export, so the desktop build and the test
//! suite keep std's exact behaviour.

#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;

/// Seconds since the Unix epoch.
#[cfg(not(target_arch = "wasm32"))]
pub fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(target_arch = "wasm32")]
pub use wasm_clock::Instant;

#[cfg(target_arch = "wasm32")]
pub fn now_unix_secs() -> u64 {
    // Date::now() is milliseconds since the epoch, as an f64.
    (js_sys::Date::now() / 1000.0).max(0.0) as u64
}

#[cfg(target_arch = "wasm32")]
mod wasm_clock {
    use std::time::Duration;

    /// Stand-in for `std::time::Instant` with the same surface the core uses
    /// (`now()` and `elapsed()`), so no call site changes.
    ///
    /// Backed by `Date::now()` rather than `performance.now()`. That trades
    /// monotonicity for having no `web-sys` dependency and no Window-versus-
    /// WorkerGlobalScope branch, which is a fair trade here: every use in this
    /// crate is import/query instrumentation reported in whole milliseconds, not
    /// anything correctness depends on. A backwards system-clock adjustment can
    /// therefore make an interval read as zero — hence the clamp below — but
    /// cannot make it negative or panic.
    #[derive(Clone, Copy, Debug)]
    pub struct Instant(f64);

    impl Instant {
        pub fn now() -> Self {
            Self(js_sys::Date::now())
        }

        pub fn elapsed(&self) -> Duration {
            let ms = (js_sys::Date::now() - self.0).max(0.0);
            Duration::from_secs_f64(ms / 1000.0)
        }
    }
}
