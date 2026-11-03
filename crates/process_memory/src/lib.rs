//! The one place Pulse talks to the C allocator.
//!
//! The workspace forbids `unsafe`, and this crate does not opt into that lint:
//! returning freed pages to the kernel has no safe API, so the call lives here
//! behind a safe function instead of spreading an exemption across the app.

/// Hand freed-but-retained pages back to the kernel.
///
/// A long task that parses thousands of files — the library scan — leaves its
/// peak in the allocator's arenas. glibc keeps those pages mapped after the
/// task ends, so the process sits at the high-water mark for the rest of the
/// session even though nothing is using it. `malloc_trim(0)` is the only way to
/// give them back.
///
/// Cheap (one pass over the arenas) and callable from any thread. A no-op on
/// targets without glibc.
pub fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // SAFETY: `malloc_trim` inspects the allocator's own arenas. It takes no
        // pointers from the caller and only releases pages that are already
        // free, so there is nothing for the caller to get wrong.
        unsafe {
            malloc_trim(0);
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}
