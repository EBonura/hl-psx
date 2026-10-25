//! PS1 CPU scratchpad access.
//!
//! The R3000A exposes 1 KiB of CPU-local memory at `0x1f80_0000` through
//! KUSEG/KSEG0. It is ordinary fast RAM, not MMIO, and cannot participate in
//! DMA. Keep GPU packets and streamed payloads in main RAM; this module is for
//! small CPU-only working sets that are hottest while DMA owns the main bus.

/// Total hardware scratchpad capacity.
pub use psx_engine::scratchpad::SIZE;

#[cfg(feature = "main-ram-render-scratch")]
#[repr(C, align(16))]
struct AlignedScratchpad([u8; SIZE]);

// Host builds and the explicit A/B baseline use an identically sized/aligned
// main-RAM block. That keeps all callers and data layouts identical: only the
// physical memory backing changes.
#[cfg(feature = "main-ram-render-scratch")]
static mut MAIN_RAM_SCRATCHPAD: AlignedScratchpad = AlignedScratchpad([0; SIZE]);

// `a0=context`, `a1=entry`, `a2=new stack top`, `a3=save area`. Preserve the
// old stack, return address, and s0 in the arena below the guarded stack floor;
// s0 is callee-saved, so it remains a reliable way back after the Rust entry
// function has used every caller-saved register.
#[cfg(target_arch = "mips")]
core::arch::global_asm!(
    ".section .text.__hlpsx_call_on_projection_stack,\"ax\",@progbits",
    ".globl __hlpsx_call_on_projection_stack",
    ".type __hlpsx_call_on_projection_stack,@function",
    ".set noreorder",
    ".ent __hlpsx_call_on_projection_stack",
    "__hlpsx_call_on_projection_stack:",
    "sw $sp, 0($7)",
    "sw $ra, 4($7)",
    "sw $16, 8($7)",
    "move $16, $7",
    "move $sp, $6",
    "jalr $5",
    "nop",
    "lw $ra, 4($16)",
    "lw $sp, 0($16)",
    "lw $16, 8($16)",
    "jr $ra",
    "nop",
    ".end __hlpsx_call_on_projection_stack",
);

#[cfg(target_arch = "mips")]
unsafe extern "C" {
    fn __hlpsx_call_on_projection_stack(
        context: *mut u8,
        entry: unsafe extern "C" fn(*mut u8),
        stack_top: *mut u8,
        save_area: *mut u32,
    );
}

/// First byte of the hot globals that live at the top of the scratchpad (see `HotCells`
/// in main.rs). The projection stack grows down from here, not from byte 1024: its
/// longest chain is 640 bytes (the SDK stack-guard proves it after every link), and
/// the bytes above it are too hot to give up.
pub const HOT_OFFSET: usize = 984;
/// Where the recyclable projection stack starts, growing down.
pub const PROJECTION_STACK_TOP: usize = HOT_OFFSET;

const PROJECTION_SAVE_OFFSET: usize = 128;
const PROJECTION_GUARD_OFFSET: usize = PROJECTION_SAVE_OFFSET + 3 * core::mem::size_of::<u32>();
// A profile-guided build inlines more into the bone chain and needs 824 bytes
// of it; the canary gave up 84 of its 116 to leave 852. The SDK's stack-guard
// proves the linked chain fits after every build, so the canary only has to
// catch what static analysis cannot see.
const PROJECTION_GUARD_END: usize = 172;
const PROJECTION_GUARD_WORDS: usize =
    (PROJECTION_GUARD_END - PROJECTION_GUARD_OFFSET) / core::mem::size_of::<u32>();
const PROJECTION_GUARD: u32 = 0x5a17_c0de;
// Both A/B builds retain the same runtime branch and both projection paths.
// Distinct non-zero values keep this byte in `.data` in either build; the
// volatile read prevents the selector from folding either path away.
static mut PROJECTION_STACK_MODE: u8 = if cfg!(feature = "main-ram-projection-stack") {
    2
} else {
    1
};

#[inline(always)]
unsafe fn base() -> *mut u8 {
    #[cfg(not(feature = "main-ram-render-scratch"))]
    {
        unsafe { psx_engine::scratchpad::base_ptr() }
    }

    #[cfg(feature = "main-ram-render-scratch")]
    {
        core::ptr::addr_of_mut!(MAIN_RAM_SCRATCHPAD).cast::<u8>()
    }
}

/// Execute one context entry on the recyclable projection stack.
///
/// Bytes 0..128 remain owned by persistent viewmodel bucket heads. The save
/// record occupies 128..140 and a 32-byte canary extends through byte 171;
/// the stack grows down from byte 1024. Returning `false` means the stack used
/// more than its audited 852-byte guarded budget, but did not reach the bucket
/// heads or the trampoline's save record.
///
/// # Safety
/// The caller must ensure the face-group bitsets at bytes 128..840 are dead,
/// `entry` follows the C ABI, and `context` remains valid until it returns.
pub unsafe fn call_on_projection_stack(
    context: *mut u8,
    entry: unsafe extern "C" fn(*mut u8),
) -> bool {
    // The A/B control retains the same non-inlined entry function but lets it
    // use the ordinary CPU stack. It needs no second main-RAM arena and makes
    // the scratchpad candidate earn back its trampoline and guard overhead.
    let mode = unsafe { core::ptr::addr_of!(PROJECTION_STACK_MODE).read_volatile() };
    if mode == 2 {
        unsafe { entry(context) };
        return true;
    }

    let arena = unsafe { base() };
    let guard = unsafe { arena.add(PROJECTION_GUARD_OFFSET).cast::<u32>() };
    let mut i = 0usize;
    while i < PROJECTION_GUARD_WORDS {
        unsafe { guard.add(i).write(PROJECTION_GUARD ^ i as u32) };
        i += 1;
    }

    #[cfg(target_arch = "mips")]
    unsafe {
        __hlpsx_call_on_projection_stack(
            context,
            entry,
            arena.add(PROJECTION_STACK_TOP),
            arena.add(PROJECTION_SAVE_OFFSET).cast::<u32>(),
        );
    }

    #[cfg(not(target_arch = "mips"))]
    unsafe {
        entry(context);
    }

    i = 0;
    while i < PROJECTION_GUARD_WORDS {
        if unsafe { guard.add(i).read() } != PROJECTION_GUARD ^ i as u32 {
            return false;
        }
        i += 1;
    }
    true
}

/// A closure and its result, handed to the stack entry through a pointer.
struct StackJob<F, R> {
    f: Option<F>,
    out: Option<R>,
}

#[inline(never)]
unsafe extern "C" fn stack_job_entry<R, F: FnOnce() -> R>(context: *mut u8) {
    // SAFETY: `context` is the `StackJob` that `run_on_projection_stack` built
    // for exactly this monomorphisation and keeps alive until the call returns.
    let job = unsafe { &mut *context.cast::<StackJob<F, R>>() };
    if let Some(f) = job.f.take() {
        job.out = Some(f());
    }
}

/// Set once a scratchpad-stack run reached its canary; later runs use the
/// ordinary stack.
static mut STACK_JOB_OVERFLOWED: bool = false;

// A/B switch: 1 runs closures on the scratchpad stack, 2 runs the same non-inlined
// entry on the ordinary stack. A volatile read keeps both paths in either build.
const JOBS_ON_SCRATCHPAD: bool = true;
static mut STACK_JOB_MODE: u8 = if JOBS_ON_SCRATCHPAD { 1 } else { 2 };

/// Run `f` with the stack pointer on the recyclable projection stack, bytes
/// 172..1024 of the scratchpad, and return its result. The same region and the
/// same limits as [`call_on_projection_stack`]: everything `f` calls must fit
/// in 852 bytes and nothing in bytes 128..840 may be live. If a run reaches
/// the canary the hardware stack is used from then on.
///
/// # Safety
/// The caller must ensure bytes 128..1024 are dead, the CPU is not already on
/// a scratchpad stack, and `f`'s deepest call chain fits in 852 bytes.
#[inline(always)]
pub unsafe fn run_on_projection_stack<R, F: FnOnce() -> R>(f: F) -> R {
    let mut job = StackJob { f: Some(f), out: None };
    if unsafe { STACK_JOB_OVERFLOWED } || unsafe { core::ptr::addr_of!(STACK_JOB_MODE).read_volatile() } == 2 {
        unsafe { stack_job_entry::<R, F>(core::ptr::addr_of_mut!(job).cast::<u8>()) };
        return match job.out {
            Some(out) => out,
            // SAFETY: the entry stores a result whenever it runs, and it always runs.
            None => unsafe { core::hint::unreachable_unchecked() },
        };
    }
    let intact = unsafe {
        call_on_projection_stack(core::ptr::addr_of_mut!(job).cast::<u8>(), stack_job_entry::<R, F>)
    };
    if !intact {
        unsafe { STACK_JOB_OVERFLOWED = true };
    }
    match job.out {
        Some(out) => out,
        // SAFETY: the entry stores a result whenever it runs, and it always runs.
        None => unsafe { core::hint::unreachable_unchecked() },
    }
}

/// Run `entry(context)` with the stack pointer at scratchpad byte
/// `stack_top`, the trampoline's 12-byte save record at `save`.
///
/// # Safety
/// `entry` follows the C ABI and its deepest call chain fits below
/// `stack_top` without reaching `save + 12`; nothing else lives in that range
/// until it returns; the CPU is not already on a scratchpad stack.
#[inline(always)]
pub unsafe fn call_on_stack(
    context: *mut u8,
    entry: unsafe extern "C" fn(*mut u8),
    stack_top: usize,
    save: usize,
) {
    #[cfg(target_arch = "mips")]
    unsafe {
        let arena = base();
        __hlpsx_call_on_projection_stack(
            context,
            entry,
            arena.add(stack_top),
            arena.add(save).cast::<u32>(),
        );
    }
    #[cfg(not(target_arch = "mips"))]
    unsafe {
        let _ = (stack_top, save);
        entry(context);
    }
}

/// Return a typed pointer at `byte_offset` within the scratchpad arena.
///
/// # Safety
/// The caller must reserve a suitably aligned, non-overlapping range large
/// enough for every element it will address through the returned pointer.
#[inline(always)]
pub unsafe fn ptr_at<T>(byte_offset: usize) -> *mut T {
    debug_assert!(byte_offset <= SIZE.saturating_sub(core::mem::size_of::<T>()));
    debug_assert!(byte_offset % core::mem::align_of::<T>() == 0);

    unsafe { base().add(byte_offset).cast::<T>() }
}

/// Establish deterministic contents before the first consumer starts.
///
/// # Safety
/// Call only when no scratchpad-backed working set is live.
pub unsafe fn clear() {
    let words = unsafe { ptr_at::<u32>(0) };
    let mut i = 0usize;
    while i < SIZE / core::mem::size_of::<u32>() {
        unsafe { words.add(i).write(0) };
        i += 1;
    }
}
