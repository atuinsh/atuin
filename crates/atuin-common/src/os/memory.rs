//! Handing memory the allocator keeps back to the operating system.

/// Return the memory that the allocator holds free to the operating system, where it can.
///
/// glibc keeps the memory it frees in per-thread arenas instead of returning it. After a burst of
/// allocation, such as a thread import, a process can then stay at its peak size for good, even
/// though most of that memory is free. `malloc_trim` returns the free pages of every arena.
///
/// It walks every arena, so it can take tens of milliseconds after a large burst. Call it on a
/// blocking thread. On other platforms and allocators, it does nothing.
pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` takes no pointers and only releases memory that glibc's allocator
        // holds free. It is safe to call from any thread at any time.
        #[allow(unsafe_code, reason = "calling glibc's malloc_trim")]
        let _ = unsafe { libc::malloc_trim(0) };
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    fn releasing_free_memory_leaves_live_allocations_intact() {
        let kept: Vec<Vec<u8>> = (0..64).map(|i| vec![i; 64 * 1024]).collect();
        let freed: Vec<Vec<u8>> = (0..64).map(|i| vec![i; 64 * 1024]).collect();
        drop(freed);

        release_free_memory();

        assert!(
            kept.iter().enumerate().all(|(i, block)| block.iter().all(|&b| usize::from(b) == i))
        );
    }
}
