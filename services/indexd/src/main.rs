// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>

//! indexd: semantic index daemon, being ported from cosmix-indexd
//! (`_plan/2026-10-20-indexd-port.md`).
//!
//! Stage 1 (dependency review) placeholder. It only names one item from each
//! heavy dependency so the build pulls in and compiles the full closure
//! (candle with nomic_bert, tokenizers on the pure-Rust regex backend,
//! sqlite-vec plus bundled SQLite). The daemon arrives in stages 2–4.

use candle_core::Device;
use candle_transformers::models::nomic_bert::NomicBertModel;
use tokenizers::Tokenizer;

fn main() {
    let _ = Device::Cpu;
    println!(
        "indexd stage-1 probe: {} {} sqlite-vec loaded={}",
        std::any::type_name::<NomicBertModel>(),
        std::any::type_name::<Tokenizer>(),
        register_sqlite_vec(),
    );
}

/// Proves sqlite-vec links against the bundled SQLite. cosmix-indexd
/// registers it the same way.
fn register_sqlite_vec() -> bool {
    type Init = unsafe extern "C" fn(
        *mut rusqlite::ffi::sqlite3,
        *mut *mut std::os::raw::c_char,
        *const rusqlite::ffi::sqlite3_api_routines,
    ) -> std::os::raw::c_int;
    // SAFETY: sqlite3_vec_init has exactly the SQLite extension entry-point
    // signature. sqlite3_auto_extension only records the pointer.
    let rc = unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<*const (), Init>(
            sqlite_vec::sqlite3_vec_init as *const (),
        )))
    };
    if rc != rusqlite::ffi::SQLITE_OK {
        return false;
    }
    rusqlite::Connection::open_in_memory()
        .and_then(|c| c.query_row("SELECT vec_version()", [], |r| r.get::<_, String>(0)))
        .is_ok()
}
