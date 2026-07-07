// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::time::{SystemTime, UNIX_EPOCH};

use zbit_rs::{
    compress_adaptive_stream_to_file, decompress_stream_file,
    decompress_stream_file_from_key_piece, StreamPackOptions,
};

#[test]
fn adaptive_stream_pack_real_file_roundtrip_and_key_piece_resume() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let inputs = [
        ("paper", root.join("papers/zbit-algorithmsResearch.md")),
        ("primary.3b", root.join("assets/primary.3b.bin")),
    ];

    for (label, input_path) in inputs {
        let input = std::fs::read(&input_path).unwrap_or_else(|e| {
            panic!(
                "read stream benchmark input {label} at {}: {e}",
                input_path.display()
            )
        });

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let pack_path = std::env::temp_dir().join(format!("zbit_stream_{label}_{stamp}.zbps"));

        let options = StreamPackOptions {
            chunk_size: 256 * 1024,
            key_piece_interval: 4,
            max_group_depth: 2,
            max_group_pieces: 4,
            carry_grouping_history: true,
            realtime_mode: true,
            wide_overfitting_circuits: true,
        };

        let stats = compress_adaptive_stream_to_file(&input, &pack_path, &options)
            .unwrap_or_else(|e| panic!("compress adaptive stream for {label}: {e}"));
        let decoded = decompress_stream_file(&pack_path)
            .unwrap_or_else(|e| panic!("decompress adaptive stream for {label}: {e}"));

        assert_eq!(decoded, input, "stream decoded bytes mismatch for {label}");

        if stats.total_chunks > stats.key_piece_interval {
            let start_chunk = stats.key_piece_interval;
            let resumed = decompress_stream_file_from_key_piece(&pack_path, start_chunk)
                .unwrap_or_else(|e| {
                    panic!(
                        "decompress adaptive stream from key piece for {label} at chunk {}: {e}",
                        start_chunk
                    )
                });
            let start_offset = start_chunk * stats.chunk_size;
            assert_eq!(
                resumed,
                input[start_offset..],
                "stream key-piece resume mismatch for {label}"
            );
        }

        std::fs::remove_file(&pack_path).ok();

        assert_eq!(stats.chunk_size, options.chunk_size);
        assert_eq!(stats.key_piece_interval, options.key_piece_interval);
        assert_eq!(stats.block_count, stats.key_piece_count);
        assert!(stats.compressed_size > 0);
    }
}
