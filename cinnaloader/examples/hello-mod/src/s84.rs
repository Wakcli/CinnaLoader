//! Section 84 demo.

use cinnaloader::packages::{self, Manifest, Package, SignatureCheck};
use cinnaloader::permissions::TrustLevel;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let manifest = Manifest::new("hello:pkg", "Hello package", "1.2.0")
        .key("hello-key")
        .trust(TrustLevel::Normal)
        .depends_on("core_lib")
        .permission("read_files");
    let package = Package::new(
        &manifest,
        vec![("greeting.txt".to_owned(), b"hi".to_vec())],
    );
    let bytes = packages::pack(&package);
    let opened = packages::open(&bytes).expect("a whole package");
    lines.push(format!(
        "packages: wrote {} bytes, round trip exact={}, contents={:?}, checksum={}",
        bytes.len(),
        opened == package && packages::pack(&opened) == bytes,
        packages::list_contents(&opened),
        packages::calculate_checksum(&opened),
    ));

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    lines.push(format!(
        "packages: bad magic -> {}",
        packages::open(&bad_magic).expect_err("refused")
    ));

    lines.push(format!(
        "packages: truncated -> {}",
        packages::open(&bytes[..bytes.len() - 3]).expect_err("refused")
    ));

    let tampered = {
        let mut copy = bytes.clone();
        let header = copy.len() - package.payload.len();
        let manifest_size = package.header.files[0].size as usize;
        copy[header + manifest_size] ^= 0xff;
        copy
    };
    lines.push(format!(
        "packages: tampered -> {}",
        packages::open(&tampered).expect_err("refused")
    ));

    let other = Package::new(
        &Manifest::new("hello:pkg", "Hello package", "1.2.0").abi(packages::ABI_MAJOR + 1),
        Vec::new(),
    );
    lines.push(format!(
        "packages: abi {} against build {} -> {}",
        packages::ABI_MAJOR + 1,
        packages::ABI_MAJOR,
        packages::open(&packages::pack(&other)).expect_err("refused")
    ));

    let fingerprint = packages::signature_of("hello-key", &package.payload);
    let mismatched = {
        let mut copy = package.clone();
        let last = copy.payload.len() - 1;
        copy.payload[last] ^= 0xff;
        copy
    };
    lines.push(format!(
        "packages: signature matched={} unknown={} mismatched={}",
        matches!(
            packages::verify_signature(&package, &[fingerprint]),
            Ok(SignatureCheck::Matched { .. })
        ),
        matches!(
            packages::verify_signature(&package, &[]),
            Ok(SignatureCheck::UnknownSigner { .. })
        ),
        matches!(
            packages::verify_signature(&mismatched, &[fingerprint]),
            Ok(SignatureCheck::Mismatched { .. })
        ),
    ));

    for version in ["1.2.0", "1.10.0", "1.2.1"] {
        let held = Package::new(
            &Manifest::new("hello:pkg", "Hello package", version),
            Vec::new(),
        );
        let _ = packages::register(held);
    }
    lines.push(format!(
        "packages: index versions={:?}, newest={:?}",
        packages::versions("hello:pkg"),
        packages::newest("hello:pkg").map(|held| held.header.version),
    ));

    let refused = packages::refusals()
        .into_iter()
        .map(|(what, _)| what)
        .collect::<Vec<_>>();
    lines.push(format!(
        "packages: client read refused={}, refusals={:?}, why={}",
        packages::read_client_package(b"x").is_err(),
        refused,
        packages::why_unknown("read_client_package").split(' ').take(6).collect::<Vec<_>>().join(" "),
    ));

    lines
}
