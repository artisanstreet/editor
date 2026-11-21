use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Counting;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static FREED: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static ALLOCATED_HERE: Cell<usize> = const { Cell::new(0) };
}

fn note(bytes: usize) {
    ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    ALLOCATED_HERE.with(|here| here.set(here.get() + bytes));
}

pub(crate) fn allocated_here() -> u64 {
    u64::try_from(ALLOCATED_HERE.with(Cell::get)).expect("invariant: a byte count fits in u64")
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        FREED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        FREED.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Heap {
    allocated: usize,
    freed: usize,
}

impl Heap {
    pub(crate) fn now() -> Self {
        Self {
            allocated: ALLOCATED.load(Ordering::Relaxed),
            freed: FREED.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn retained_since(self, earlier: Self) -> u64 {
        let live = |heap: Self| heap.allocated.saturating_sub(heap.freed);
        u64::try_from(live(self).saturating_sub(live(earlier)))
            .expect("invariant: a heap size fits in u64")
    }
}
