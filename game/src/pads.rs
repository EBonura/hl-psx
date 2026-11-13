//! The game's one pad reader: `psx_pad::Controllers` on port 1, polled from the title, the
//! menus and the play loop alike. The first poll of a pad runs the analog handshake (a few
//! frames, once per plug-in), a garbled packet returns the last clean state, and an empty
//! port is probed once in 60 polls.

use psx_io::periph::ControllerPort;
use psx_pad::{AnalogRequirement, Controllers, PadState, Port};

static mut PADS: Controllers = Controllers::new();

/// Poll port 1 once and return its latest clean state.
pub fn poll() -> PadState {
    // SAFETY: the game is single-threaded and polls the pad only through this function; a
    // memory-card transfer borrows the port through its own token and never overlaps a poll.
    unsafe {
        let mut port = ControllerPort::steal();
        *(*core::ptr::addr_of_mut!(PADS)).poll(&mut port, Port::One)
    }
}

/// What port 1 held at the last poll: analog and locked, a digital pad, or nothing.
pub fn analog() -> AnalogRequirement {
    // SAFETY: as in `poll`.
    unsafe { (*core::ptr::addr_of!(PADS)).analog(Port::One) }
}
