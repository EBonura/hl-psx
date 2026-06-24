//! PSoXide host-side telemetry hooks for headless screenshots and profiling.
//!
//! The event IDs come from the shared `psx-telemetry` crate. The writes are
//! gated behind `emulator-telemetry`, so normal PS1 builds only pay no-op calls.

pub use psx_telemetry::{counter, stage, task};

const EVENT_KIND_FRAME_BEGIN: u8 = 1;
const EVENT_KIND_STAGE_BEGIN: u8 = 2;
const EVENT_KIND_STAGE_END: u8 = 3;
const EVENT_KIND_COUNTER: u8 = 4;
const EVENT_KIND_TASK_BEGIN: u8 = 5;
const EVENT_KIND_TASK_END: u8 = 6;

#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
const EVENT_ADDR: *mut u32 = 0xBF80_2F00 as *mut u32;
#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
const VALUE_ADDR: *mut u32 = 0xBF80_2F04 as *mut u32;
#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
const LOG_ADDR: *mut u32 = 0xBF80_2F0C as *mut u32;

#[inline(always)]
pub fn frame_begin(frame: u32) {
    emit_value(frame);
    emit_event(EVENT_KIND_FRAME_BEGIN, 0);
}

#[inline(always)]
pub fn stage_begin(stage_id: u16) {
    emit_event(EVENT_KIND_STAGE_BEGIN, stage_id);
}

#[inline(always)]
pub fn stage_end(stage_id: u16) {
    emit_event(EVENT_KIND_STAGE_END, stage_id);
}

#[inline(always)]
pub fn counter(counter_id: u16, value: u32) {
    emit_value(value);
    emit_event(EVENT_KIND_COUNTER, counter_id);
}

#[inline(always)]
pub fn task_begin(task_id: u16) {
    emit_event(EVENT_KIND_TASK_BEGIN, task_id);
}

#[inline(always)]
pub fn task_end(task_id: u16) {
    emit_event(EVENT_KIND_TASK_END, task_id);
}

#[inline(always)]
pub fn debug_log(message: &str) {
    debug_bytes(message.as_bytes());
    debug_byte(b'\n');
}

#[inline(always)]
fn debug_bytes(bytes: &[u8]) {
    for &byte in bytes {
        debug_byte(byte);
    }
}

#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
#[inline(always)]
fn encode_event(kind: u8, id: u16) -> u32 {
    ((kind as u32) << 24) | id as u32
}

#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
#[inline(always)]
fn emit_value(value: u32) {
    unsafe {
        core::ptr::write_volatile(VALUE_ADDR, value);
    }
}

#[cfg(not(all(target_arch = "mips", feature = "emulator-telemetry")))]
#[inline(always)]
fn emit_value(_value: u32) {}

#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
#[inline(always)]
fn debug_byte(byte: u8) {
    unsafe {
        core::ptr::write_volatile(LOG_ADDR, byte as u32);
    }
}

#[cfg(not(all(target_arch = "mips", feature = "emulator-telemetry")))]
#[inline(always)]
fn debug_byte(_byte: u8) {}

#[cfg(all(target_arch = "mips", feature = "emulator-telemetry"))]
#[inline(always)]
fn emit_event(kind: u8, id: u16) {
    unsafe {
        core::ptr::write_volatile(EVENT_ADDR, encode_event(kind, id));
    }
}

#[cfg(not(all(target_arch = "mips", feature = "emulator-telemetry")))]
#[inline(always)]
fn emit_event(_kind: u8, _id: u16) {}
