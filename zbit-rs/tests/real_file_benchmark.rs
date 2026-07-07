// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::time::{SystemTime, UNIX_EPOCH};

use zbit_rs::{compress_adaptive_to_file, decompress_file};

#[test]
fn adaptive_pack_real_file_roundtrip_and_size_guard() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let inputs = [
        ("paper", root.join("papers/zbit-algorithmsResearch.md")),
        ("primary.3b", root.join("assets/primary.3b.bin")),
    ];

    for (label, input_path) in inputs {
        let input = std::fs::read(&input_path).unwrap_or_else(|e| {
            panic!(
                "read benchmark input {label} at {}: {e}",
                input_path.display()
            )
        });

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let pack_path = std::env::temp_dir().join(format!("zbit_real_file_{label}_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &pack_path)
            .unwrap_or_else(|e| panic!("compress adaptive pack for {label}: {e}"));
        let decoded = decompress_file(&pack_path)
            .unwrap_or_else(|e| panic!("decompress adaptive pack for {label}: {e}"));
        std::fs::remove_file(&pack_path).ok();

        assert_eq!(decoded, input, "decoded bytes mismatch for {label}");

        // Adaptive method should never be worse than raw-copy candidate chosen by the same logic.
        assert!(
            stats.compressed_size <= stats.raw_candidate_bytes,
            "compressed size {} should be <= raw candidate {} for {}",
            stats.compressed_size,
            stats.raw_candidate_bytes,
            label
        );
    }
}
