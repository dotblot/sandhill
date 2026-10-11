//! Giving freed memory back to the system.
//!
//! Opening or saving a notebook moves tens of MB through short-lived
//! buffers. glibc and macOS's malloc keep freed memory for reuse, so without
//! this the daemon's footprint stays at the high-water mark of the largest
//! notebook it has handled, although none of that memory is in use.

/// Work that moved more than this many bytes is followed by a release.
pub const LARGE: usize = 1 << 20;

/// Call once when the daemon starts.
pub fn configure() {
    // glibc raises its mmap threshold (up to 32 MB) after a large buffer is
    // freed, so later multi-MB buffers land in thread arenas whose tops are
    // never trimmed. A fixed threshold keeps every large buffer in its own
    // mapping, returned to the kernel on free.
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: mallopt only changes allocator tuning.
    unsafe {
        libc::mallopt(libc::M_MMAP_THRESHOLD, 256 * 1024);
    }
}

pub fn release_free_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only returns free pages to the kernel.
    unsafe {
        libc::malloc_trim(0);
    }
    #[cfg(target_os = "macos")]
    // SAFETY: a null zone means every zone; the call only releases free
    // memory (it is what the system calls under memory pressure).
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: usize) -> usize;
}
