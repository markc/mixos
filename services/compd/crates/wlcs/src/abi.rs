// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! C layout of the public wlcs integration ABI (display server v2, input v1).
//! No wlcs code is linked into the adapter.
use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
pub struct Integration {
    pub version: u32,
    pub create: unsafe extern "C" fn(c_int, *const *const c_char) -> *mut DisplayServer,
    pub destroy: unsafe extern "C" fn(*mut DisplayServer),
}
#[repr(C)]
pub struct DisplayServer {
    pub version: u32,
    pub start: unsafe extern "C" fn(*mut DisplayServer),
    pub stop: unsafe extern "C" fn(*mut DisplayServer),
    pub socket: unsafe extern "C" fn(*mut DisplayServer) -> c_int,
    pub place: unsafe extern "C" fn(*mut DisplayServer, *mut c_void, *mut c_void, c_int, c_int),
    pub pointer: unsafe extern "C" fn(*mut DisplayServer) -> *mut Pointer,
    pub touch: unsafe extern "C" fn(*mut DisplayServer) -> *mut Touch,
    pub descriptor: unsafe extern "C" fn(*const DisplayServer) -> *const Descriptor,
}
#[repr(C)]
pub struct Descriptor {
    pub version: u32,
    pub count: usize,
    pub extensions: *const Extension,
}
#[repr(C)]
pub struct Extension {
    pub name: *const c_char,
    pub version: u32,
}
#[repr(C)]
pub struct Pointer {
    pub version: u32,
    pub absolute: unsafe extern "C" fn(*mut Pointer, i32, i32),
    pub relative: unsafe extern "C" fn(*mut Pointer, i32, i32),
    pub up: unsafe extern "C" fn(*mut Pointer, i32),
    pub down: unsafe extern "C" fn(*mut Pointer, i32),
    pub destroy: unsafe extern "C" fn(*mut Pointer),
}
#[repr(C)]
pub struct Touch {
    pub version: u32,
    pub down: unsafe extern "C" fn(*mut Touch, i32, i32),
    pub motion: unsafe extern "C" fn(*mut Touch, i32, i32),
    pub up: unsafe extern "C" fn(*mut Touch),
    pub destroy: unsafe extern "C" fn(*mut Touch),
}
