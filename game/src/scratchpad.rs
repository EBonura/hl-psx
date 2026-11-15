//! PS1 CPU scratchpad access: `psx_goldsrc::scratchpad` with this game's projection-stack canary.

pub use psx_goldsrc::scratchpad::*;

/// Bytes of the scratchpad below which the projection stack's canary ends: the stack gets the
/// rest of the 1 KiB (`SIZE - PROJECTION_GUARD_END`), as proved after every link.
const PROJECTION_GUARD_END: usize = 172;

/// Execute one context entry on the recyclable projection stack; see
/// `psx_goldsrc::scratchpad::call_on_projection_stack`.
///
/// # Safety
/// As `psx_goldsrc::scratchpad::call_on_projection_stack`.
#[inline(always)]
pub unsafe fn call_on_projection_stack(
    context: *mut u8,
    entry: unsafe extern "C" fn(*mut u8),
) -> bool {
    unsafe {
        psx_goldsrc::scratchpad::call_on_projection_stack::<PROJECTION_GUARD_END>(context, entry)
    }
}
