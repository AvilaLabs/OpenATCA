#![allow(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
//! Test-only allocator observer and controls. Shell (B3): system allocation and counters.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOC: AtomicU64 = AtomicU64::new(0);
static ZEROED: AtomicU64 = AtomicU64::new(0);
static REALLOC: AtomicU64 = AtomicU64::new(0);
static COUNTER_OVERFLOW: AtomicBool = AtomicBool::new(false);

struct CountingSystem;

fn increment(counter: &AtomicU64) {
    if counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .is_err()
    {
        COUNTER_OVERFLOW.store(true, Ordering::Relaxed);
    }
}

// SAFETY: each allocation operation is forwarded to System with the caller's unchanged layout,
// pointer, and size; dealloc forwards the matching pointer/layout pair unchanged. The hook uses
// only non-panicking atomics before forwarding and does not allocate, lock, access TLS, or re-enter.
unsafe impl GlobalAlloc for CountingSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            increment(&ALLOC);
        }
        // SAFETY: `layout` is passed unchanged from the caller to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            increment(&ZEROED);
        }
        // SAFETY: `layout` is passed unchanged from the caller to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            increment(&REALLOC);
        }
        // SAFETY: `ptr` and `layout` are the original allocation pair supplied to this call.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` and `layout` are forwarded unchanged from the allocator caller.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingSystem = CountingSystem;

// A7.2: cold report, one snapshot per measured window.
#[derive(Clone, Copy)]
pub struct Counts {
    pub alloc: u64,
    pub zeroed: u64,
    pub realloc: u64,
    pub overflow: bool,
}

fn test_layout() -> Option<Layout> {
    Layout::from_size_align(64, 8).ok()
}

pub fn start() -> bool {
    if ARMED.load(Ordering::SeqCst)
        || ALLOC.load(Ordering::SeqCst) != 0
        || ZEROED.load(Ordering::SeqCst) != 0
        || REALLOC.load(Ordering::SeqCst) != 0
        || COUNTER_OVERFLOW.load(Ordering::SeqCst)
    {
        return false;
    }
    ARMED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

pub fn stop() -> Option<Counts> {
    if !ARMED.swap(false, Ordering::SeqCst) {
        return None;
    }
    Some(Counts {
        alloc: ALLOC.swap(0, Ordering::SeqCst),
        zeroed: ZEROED.swap(0, Ordering::SeqCst),
        realloc: REALLOC.swap(0, Ordering::SeqCst),
        overflow: COUNTER_OVERFLOW.swap(false, Ordering::SeqCst),
    })
}

/// Positive control through the standard global allocator API, while measurement is armed.
pub fn control_alloc() -> bool {
    let Some(layout) = test_layout() else {
        return false;
    };
    // SAFETY: the layout is valid; a non-null pointer is deallocated with the same layout.
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return false;
    }
    std::hint::black_box(ptr);
    // SAFETY: `ptr` was allocated through the global API for exactly this layout.
    unsafe { std::alloc::dealloc(ptr, layout) };
    true
}

/// Positive control through the standard global allocator API, while measurement is armed.
pub fn control_zeroed() -> bool {
    let Some(layout) = test_layout() else {
        return false;
    };
    // SAFETY: the layout is valid; a non-null pointer is deallocated with the same layout.
    let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
    if ptr.is_null() {
        return false;
    }
    std::hint::black_box(ptr);
    // SAFETY: `ptr` was allocated through the global API for exactly this layout.
    unsafe { std::alloc::dealloc(ptr, layout) };
    true
}

/// Positive control through the standard global allocator API, while measurement is armed.
pub fn control_realloc() -> bool {
    let Some(layout) = test_layout() else {
        return false;
    };
    let Some(grown_layout) = Layout::from_size_align(128, 8).ok() else {
        return false;
    };
    // SAFETY: the layout is valid; the returned pointer follows the standard global allocator route.
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return false;
    }
    // SAFETY: `ptr` came from the standard global allocator for `layout`; the new size is nonzero.
    let grown = unsafe { std::alloc::realloc(ptr, layout, 128) };
    if grown.is_null() {
        // SAFETY: realloc failure leaves the original allocation valid.
        unsafe { std::alloc::dealloc(ptr, layout) };
        return false;
    }
    std::hint::black_box(grown);
    // SAFETY: `grown` is the successful result of realloc for `grown_layout`.
    unsafe { std::alloc::dealloc(grown, grown_layout) };
    true
}

/// Negative control: force one standard global allocation inside an armed core window.
pub fn inject_core_allocation() -> bool {
    let Some(layout) = test_layout() else {
        return false;
    };
    // SAFETY: the layout is valid; a non-null pointer is deallocated with the same layout.
    let ptr = unsafe { std::alloc::alloc(layout) };
    if ptr.is_null() {
        return false;
    }
    std::hint::black_box(ptr);
    // SAFETY: `ptr` was allocated through the global API for exactly this layout.
    unsafe { std::alloc::dealloc(ptr, layout) };
    true
}

#[cfg(test)]
// REQ: OA-TRK-327
#[test]
fn standard_global_allocation_controls_are_observed() {
    assert!(start(), "allocator measurement did not arm");
    let alloc_ok = control_alloc();
    let zeroed_ok = control_zeroed();
    let realloc_ok = control_realloc();
    let counts = stop().expect("allocator measurement did not disarm");
    assert!(
        alloc_ok && zeroed_ok && realloc_ok,
        "a control allocation failed"
    );
    assert!(!counts.overflow, "allocator counter overflow");
    assert!(
        counts.alloc >= 1,
        "standard alloc API did not hit global hook"
    );
    assert!(
        counts.zeroed >= 1,
        "standard alloc_zeroed API did not hit global hook"
    );
    assert!(
        counts.realloc >= 1,
        "standard realloc API did not hit global hook"
    );
}
