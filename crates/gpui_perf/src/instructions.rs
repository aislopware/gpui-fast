//! Instructions a thread retires, counted by the CPU: unlike CPU time, the
//! count does not depend on the core a thread ran on, the clock it ran at or
//! what else the machine was doing, so two builds can be compared on a busy
//! machine.

/// Instructions the calling thread has retired so far, where the CPU counts
/// them for it. Unlike CPU time, this does not depend on the core the thread
/// ran on or the clock it ran at.
pub fn main_thread_instructions() -> Option<u64> {
    // Exported by libsystem_kernel, though not in the SDK's headers: the
    // kernel's per-thread count of the CPU's performance counters.
    unsafe extern "C" {
        fn thread_selfcounts(kind: libc::c_int, buffer: *mut u64, size: usize) -> libc::c_int;
    }
    // Kind 1 is instructions and cycles.
    let mut counts = [0u64; 2];
    // SAFETY: the buffer holds the two counters kind 1 writes.
    let status = unsafe { thread_selfcounts(1, counts.as_mut_ptr(), size_of_val(&counts)) };
    (status == 0).then_some(counts[0])
}
