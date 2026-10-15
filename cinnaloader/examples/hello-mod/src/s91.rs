//! Section 91 demo.

use cinnaloader::network_debug::{self, Capture, Direction, Filter, Record};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    network_debug::reset();
    network_debug::set_capacity(64);
    network_debug::start_recording();
    let sample = [
        (Direction::ToServer, 7u32, 10u64, 1000u64, None),
        (Direction::ToServer, 7, 20, 1100, Some("keepalive")),
        (Direction::ToServer, 7, 30, 2000, None),
        (Direction::ToClient, 8, 200, 2000, None),
        (Direction::ToServer, 8, 40, 2300, None),
    ];
    for (direction, id, size, at, label) in sample {
        network_debug::record(direction, id, size, at, label);
    }
    lines.push(format!(
        "log: {} records in a ring of {}",
        network_debug::count(),
        network_debug::capacity()
    ));

    let counts = network_debug::get_packet_counts()
        .iter()
        .map(|(id, count)| format!("{id}x{count}"))
        .collect::<Vec<_>>()
        .join(" ");
    let sizes = network_debug::get_packet_sizes()
        .iter()
        .map(|size| format!("id{} n{} bytes{} mean{:.1} p95 {}", size.id, size.count, size.total_bytes, size.mean, size.p95))
        .collect::<Vec<_>>()
        .join(" | ");
    let stats = network_debug::get_statistics();
    lines.push(format!(
        "counts: [{counts}] sizes: [{sizes}] total {} bytes in {} records, {:.0}/s over 2s",
        stats.total_bytes,
        stats.count,
        network_debug::rate(std::time::Duration::from_secs(2))
    ));

    let composed = Filter::new()
        .direction(Direction::ToServer)
        .id(7)
        .min_size(15)
        .between(1100, 2000);
    let matched = network_debug::filter_packets(&composed);
    lines.push(format!(
        "filter: to_server id=7 min=15 time=1100..2000 -> {} (sizes {:?})",
        matched.len(),
        matched.iter().map(|record| record.size).collect::<Vec<_>>()
    ));

    let capture = network_debug::capture();
    let json = network_debug::save_recording(&capture);
    let back = network_debug::load_recording(&json);
    lines.push(format!(
        "capture: json {} bytes, reimport equal {}",
        json.len(),
        back.as_ref() == Some(&capture)
    ));

    let mut other = Capture::default();
    for (index, record) in capture.records.iter().enumerate() {
        if index != 2 {
            other.records.push(record.clone());
        }
    }
    other.records.push(Record::new(Direction::ToClient, 9, 12, 3000));
    let diff = network_debug::compare_recordings(&capture, &other);
    lines.push(format!(
        "diff: removed {:?} added {:?}",
        diff.removed_ids(),
        diff.added_ids()
    ));

    let mut order = Vec::new();
    let report = network_debug::replay(&capture, &mut |record| order.push(record.id));
    lines.push(format!("replay: order {order:?} -> {}", report.n_of_m()));

    let mut paused_order = Vec::new();
    let first = network_debug::replay(&capture, &mut |record| {
        paused_order.push(record.id);
        if paused_order.len() == 3 {
            network_debug::pause_replay();
        }
    });
    let rest = network_debug::resume_replay(&mut |record| paused_order.push(record.id));
    lines.push(format!(
        "replay: paused {} then resumed {} order {:?}",
        first.n_of_m(),
        rest.n_of_m(),
        paused_order
    ));

    let buckets = network_debug::get_bandwidth_history()
        .iter()
        .map(|bucket| format!("s{}:{}b/{}p", bucket.second, bucket.bytes, bucket.packets))
        .collect::<Vec<_>>()
        .join(" ");
    let peak = network_debug::peak_bucket();
    lines.push(format!(
        "bandwidth: [{buckets}] peak s{}={} bytes",
        peak.map_or(0, |bucket| bucket.second),
        peak.map_or(0, |bucket| bucket.bytes)
    ));

    lines.push(format!(
        "refusals: {} | client packets {} latency {} connection {}",
        network_debug::refusals().len(),
        network_debug::get_client_packets().len(),
        network_debug::get_client_latency().is_some(),
        network_debug::get_client_connection().is_some()
    ));

    lines
}
