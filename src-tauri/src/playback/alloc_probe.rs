//! A counting global allocator for the lib's TEST binary only (B5a): proves the audio callback
//! allocates nothing.
//!
//! It is the system allocator plus one thread-local check. Counting is armed per thread inside
//! [`count`], so the other tests running in parallel are neither counted nor slowed beyond that
//! check. The thread-local is a `const`-initialised `Cell` with no destructor, so reading it
//! never allocates (no lazy init, no destructor registration) and the allocator cannot recurse.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<usize> = const { Cell::new(0) };
}

pub struct Counting;

fn note() {
    let _ = ARMED.try_with(|a| {
        if a.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

// SAFETY: every call is forwarded unchanged to `System`; the bookkeeping touches only
// thread-locals that never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Run `f` on this thread and return how many allocations, reallocations and frees it made.
pub fn count(f: impl FnOnce()) -> usize {
    COUNT.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    COUNT.with(|c| c.get())
}

/// The probe is really installed: a `Vec` made under it is counted.
pub fn armed_works() -> bool {
    count(|| {
        let v: Vec<u8> = Vec::with_capacity(std::hint::black_box(64));
        std::hint::black_box(&v);
    }) >= 1
}
