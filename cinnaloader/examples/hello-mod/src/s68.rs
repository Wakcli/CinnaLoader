//! Section 68 demo.

use cinnaloader::memory;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let mut arena = memory::Arena::new(64);
    let a = arena.alloc(16, 8).expect("16 bytes aligned to 8");
    let b = arena.alloc(8, 16).expect("8 bytes aligned to 16");
    lines.push(format!(
        "memory | arena alloc(16,8) -> offset {a}, alloc(8,16) -> offset {b} (aligned), used {} of 64",
        arena.used()
    ));
    lines.push(format!(
        "memory | arena alloc(64,1) in a 64-byte block answers {:?}",
        arena.alloc(64, 1)
    ));

    let mut slab = memory::Slab::new(64);
    let first = slab.alloc(16, 8).expect("first block");
    let second = slab.alloc(16, 8).expect("second block");
    let freed = slab.free(first).expect("a live block");
    let again = slab.alloc(16, 8).expect("the freed block");
    lines.push(format!(
        "memory | slab first {first} second {second}, freed {freed} bytes, reused at {again} (same as first: {})",
        first == again
    ));

    memory::register_allocation_tracker("hello", Some(1024));
    let mesh = memory::track("mesh", 256, "render").expect("within budget");
    let tex = memory::track("texture", 128, "render").expect("within budget");
    lines.push(format!(
        "memory | ledger after 256+128 bytes: current {} peak {} total {}",
        memory::get_allocated(),
        memory::get_peak_usage(),
        memory::get_total_bytes()
    ));
    lines.push(format!(
        "memory | budget 1024 refuses 4096 more: {:?}",
        memory::track("atlas", 4096, "render")
    ));
    memory::untrack(tex);
    let leaks: Vec<String> = memory::get_leak_report()
        .into_iter()
        .map(|leak| leak.name)
        .collect();
    lines.push(format!(
        "memory | after untrack: current {} of {} live, leak report names {leaks:?}",
        memory::get_allocated(),
        memory::get_allocation_count()
    ));

    let mut pool = memory::Pool::new(8, 2);
    let slot = pool.acquire().expect("a free slot");
    let live = pool.live_count();
    pool.release(slot).expect("a live slot");
    lines.push(format!(
        "memory | pool live after acquire {live}, after release {}, capacity {}",
        pool.live_count(),
        pool.capacity()
    ));

    lines.push(format!(
        "memory | client heap {:?}, client gc {:?}, request_gc {}: all refused",
        memory::get_client_heap_info(),
        memory::get_gc_info(),
        memory::request_gc()
    ));
    let _ = mesh;
    lines
}
