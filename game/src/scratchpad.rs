//! PS1 CPU scratchpad access: `psx_goldsrc::scratchpad` with this game's projection-stack canary.

pub use psx_goldsrc::scratchpad::*;

/// Bytes of the scratchpad below which the projection stack's canary ends: the stack gets the
/// rest of the 1 KiB as `ScratchpadStack<PROJECTION_GUARD_END, SIZE>`, proved after every link.
const PROJECTION_GUARD_END: usize = 172;

/// Run `f` on the recyclable projection stack; see
/// `psx_goldsrc::scratchpad::call_on_projection_stack`.
///
/// # Safety
/// As `psx_goldsrc::scratchpad::call_on_projection_stack`.
#[inline(always)]
pub unsafe fn call_on_projection_stack<F: FnOnce()>(f: F) -> bool {
    unsafe { psx_goldsrc::scratchpad::call_on_projection_stack::<PROJECTION_GUARD_END, F>(f) }
}
