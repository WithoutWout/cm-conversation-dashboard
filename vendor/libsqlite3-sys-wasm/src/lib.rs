//! wasm32 stand-in for `libsqlite3-sys`, used via `[patch.crates-io]`.
//!
//! `sqlite-wasm-rs` is already API-compatible with `libsqlite3-sys`: it exports
//! the bindgen symbols, `ErrorCode`/`Error`, `SQLITE_STATIC`/`SQLITE_TRANSIENT`,
//! and the `Default` impls for the vtab structs. So this is a pure re-export —
//! and it must stay one, since the orphan rule blocks adding impls here.
//!
//! The sqlite3 object code itself is linked in by `sqlite-wasm-rs`
//! (`links = "wsqlite3"`), so this crate needs no build script.
#![allow(non_snake_case, non_camel_case_types)]

pub use sqlite_wasm_rs::*;
