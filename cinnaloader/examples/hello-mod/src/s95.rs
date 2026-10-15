//! Section 95 demo: the notification queue, dedup, channels, the history ring, unread and the
//! refused client toast.

use cinnaloader::notifications::{self, History, Level, Notification};

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    notifications::dismiss_all();
    notifications::clear_history();

    let _ = notifications::show_info("hello:u1", "One", "first");
    let _ = notifications::show_success("hello:u2", "Two", "second");
    let unread_before = notifications::unread_count();
    notifications::dismiss("hello:u1");
    let unread_after = notifications::unread_count();
    out.push(format!(
        "unread count: before={} after dismiss of hello:u1={}",
        unread_before, unread_after
    ));

    notifications::dismiss_all();
    notifications::clear_history();
    let _ = notifications::show(Notification::new("queue:info", "i", "b").at(100));
    let _ = notifications::show(Notification::new("queue:err-late", "e", "b").level(Level::Error).at(300));
    let _ = notifications::show(Notification::new("queue:err-early", "e", "b").level(Level::Error).at(200));
    let _ = notifications::show(Notification::new("queue:warn", "w", "b").level(Level::Warning).at(50));
    let _ = notifications::show(Notification::new("queue:warn-z", "w", "b").level(Level::Warning).at(50));
    let order: Vec<String> = notifications::get_active().iter().map(|n| n.id.clone()).collect();
    out.push(format!(
        "queue rule level(>severity) then time then id: {:?}",
        order
    ));

    let first = notifications::show(Notification::new("hello:dup", "first", "v1"));
    let again = notifications::show(Notification::new("hello:dup", "first", "v2"));
    let queued = notifications::get_active().iter().filter(|n| n.id == "hello:dup").count();
    out.push(format!(
        "dup id: first push={:?} second push={:?} entries with that id={}",
        first, again, queued
    ));

    let _ = notifications::set_channel_enabled("hello:muted", false);
    let dropped_before = notifications::dropped_count();
    let outcome = notifications::show(Notification::new("hello:m1", "m", "x").channel("hello:muted"));
    let dropped_after = notifications::dropped_count();
    out.push(format!(
        "channel off: push={:?} dropped+{} dropped_total={}",
        outcome,
        dropped_after - dropped_before,
        dropped_after
    ));

    let mut ring = History::new(3);
    for index in 0..3 {
        let _ = ring.push(Notification::new(&format!("hello:r{index}"), "r", "b"));
    }
    let over = ring.push(Notification::new("hello:r3", "r", "b"));
    out.push(format!(
        "history ring cap 3: over the cap dropped {:?}, len={}, oldest now {:?}",
        over.map(|n| n.id),
        ring.len(),
        ring.get(0).map(|n| n.id)
    ));

    let shown = notifications::show_toast(&Notification::new("hello:t", "t", "b"));
    out.push(format!(
        "client toast refused={} | {}",
        shown,
        notifications::why_unknown("toast")
    ));

    out
}
