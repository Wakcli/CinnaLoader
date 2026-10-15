//! Section 75: real byte patching over a buffer the mod owns, the ledger that records it, and
//! the client's own code refused.

use cinnaloader::patches::{self, InjectionPoint, Patch, Pattern, Transformer};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let image = [0x48u8, 0x8b, 0x01, 0xc3, 0x48, 0x8b, 0x02, 0xc3, 0x90];
    let Ok(wildcard) = Pattern::parse("48 8b ? c3") else {
        return lines;
    };
    let found = patches::find_all(&image, &wildcard);
    lines.push(format!(
        "pattern {} over {} bytes: {} match offsets [{}]",
        wildcard,
        image.len(),
        found.len(),
        found
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let mut buffer = image;
    let patch = Patch::new(
        "hello:prologue",
        Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
        vec![0x90, 0x90, 0x90, 0x90],
    )
    .target("hello:image");

    let Ok(handle) = patches::apply(&mut buffer, &patch, 0) else {
        lines.push("apply refused".to_owned());
        return lines;
    };
    lines.push(format!(
        "applied {} at offset 0: {} -> {} | verify={} | ledger holds {} patch, original={}",
        patch.id,
        hex(&[0x48, 0x8b, 0x01, 0xc3]),
        hex(&buffer[0..4]),
        patches::verify(&buffer, handle),
        patches::applied().len(),
        hex(&patches::original_at(handle).unwrap_or_default())
    ));

    let reverted = patches::revert(&mut buffer, handle);
    lines.push(format!(
        "revert({handle})={reverted} restored byte for byte: {} | buffer {} == image {}",
        buffer == image,
        hex(&buffer),
        hex(&image)
    ));

    let wrong = Patch::new(
        "hello:wrong",
        Pattern::new(&[0x48, 0x8b, 0x02, 0xc3]),
        vec![0xcc, 0xcc, 0xcc, 0xcc],
    );
    match patches::apply(&mut buffer, &wrong, 0) {
        Err(patches::PatchError::PatternMismatch { at, expected, found }) => lines.push(format!(
            "mismatch at offset {at} refused: expected {} found {}",
            hex(&expected),
            hex(&found)
        )),
        other => lines.push(format!("unexpected mismatch result: {other:?}")),
    }

    let mut chain_buffer = image;
    let first = Patch::new(
        "hello:a",
        Pattern::new(&[0x48, 0x8b, 0x01, 0xc3]),
        vec![0x90, 0x90, 0x90, 0x90],
    )
    .target("hello:image");
    let second = Patch::new(
        "hello:b",
        Pattern::new(&[0x90, 0x90, 0x48, 0x8b]),
        vec![0xcc, 0xcc, 0xcc, 0xcc],
    )
    .target("hello:image");
    let steps = patches::apply_all(&mut chain_buffer, &[first, second]);
    let summary = steps
        .iter()
        .map(|step| match (&step.reason, step.at) {
            (None, Some(at)) => format!("{} applied at {at}", step.id),
            (Some(reason), Some(at)) => format!("{} skipped at {at}: {reason}", step.id),
            (Some(reason), None) => format!("{} skipped: {reason}", step.id),
            (None, None) => format!("{} applied", step.id),
        })
        .collect::<Vec<_>>()
        .join(" | ");
    lines.push(format!("chain over {} patches: {summary}", steps.len()));

    for (what, why) in patches::refusals() {
        lines.push(format!("refusal {what}: {why}"));
    }
    let refused = [
        patches::register_injection_point(InjectionPoint::BeforeCall, "net.minecraft.Client.tick"),
        patches::register_redirect("Client.tick", "onTick", "onTickPatched"),
        patches::register_overwrite("Client.tick", "return"),
        patches::register_mixin("net.minecraft.Client"),
        patches::patch_client_binary("/proc/self/exe"),
        patches::patch_client_function("Client.tick"),
        patches::patch_client_class("net.minecraft.Client"),
        patches::patch_client_memory("Client"),
    ];
    lines.push(format!(
        "entry points refused: injection_point={} redirect={} overwrite={} mixin={} binary={} function={} class={} memory={}",
        refused[0], refused[1], refused[2], refused[3], refused[4], refused[5], refused[6], refused[7]
    ));

    let _ = patches::register_transformer(Transformer::new("hello:rename", "old_name", "new_name"));
    let report = patches::apply_transformations("let old_name = old_name + 1;");
    lines.push(format!(
        "transformer {} renamed {} occurrence(s): '{}' | export first line '{}'",
        report.applied.join(","),
        report.count,
        report
            .renames
            .iter()
            .map(|rename| format!("{} -> {}", rename.from, rename.to))
            .collect::<Vec<_>>()
            .join(", "),
        patches::export_report().lines().next().unwrap_or("")
    ));
    let _ = patches::unregister_transformer("hello:rename");

    lines
}
