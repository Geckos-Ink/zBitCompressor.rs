// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::env;
use std::fs;
use std::time::{Instant, SystemTime};

use zbit_rs::{compress_adaptive_to_file, decompress_file};

fn format_timestamp_local(now: SystemTime) -> String {
    let datetime: chrono_like::DateTimeParts = now.into();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        datetime.year,
        datetime.month,
        datetime.day,
        datetime.hour,
        datetime.minute,
        datetime.second
    )
}

mod chrono_like {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, Copy)]
    pub struct DateTimeParts {
        pub year: i32,
        pub month: u32,
        pub day: u32,
        pub hour: u32,
        pub minute: u32,
        pub second: u32,
    }

    impl From<SystemTime> for DateTimeParts {
        fn from(value: SystemTime) -> Self {
            let dur = value
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::from_secs(0));
            let secs = dur.as_secs() as i64;

            /*
             * UTC-like formatting without external crates.
             * Sufficient for benchmark report timestamping.
             */
            let days = secs.div_euclid(86_400);
            let secs_of_day = secs.rem_euclid(86_400);

            let (year, month, day) = civil_from_days(days);
            let hour = (secs_of_day / 3600) as u32;
            let minute = ((secs_of_day % 3600) / 60) as u32;
            let second = (secs_of_day % 60) as u32;

            Self {
                year,
                month,
                day,
                hour,
                minute,
                second,
            }
        }
    }

    fn civil_from_days(z: i64) -> (i32, u32, u32) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = mp + if mp < 10 { 3 } else { -9 };
        let year = y + if m <= 2 { 1 } else { 0 };

        (year as i32, m as u32, d as u32)
    }
}

fn read_status_kib(field: &str) -> Option<u64> {
    let content = fs::read_to_string("/proc/self/status").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(field) {
            let value = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(value);
        }
    }
    None
}

fn format_opt_u64(value: Option<u64>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_opt_delta(before: Option<u64>, after: Option<u64>) -> String {
    match (before, after) {
        (Some(a), Some(b)) => b.saturating_sub(a).to_string(),
        _ => "n/a".to_string(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "../papers/zbit-algorithmsResearch.md".to_string());
    let pack_path = env::args()
        .nth(2)
        .unwrap_or_else(|| "benchmark_algorithmsResearch.zbpk".to_string());
    let report_path = env::args()
        .nth(3)
        .unwrap_or_else(|| "benchmark_latest.txt".to_string());

    let input = fs::read(&input_path)?;
    let rss_before_compress = read_status_kib("VmRSS:");

    let t0 = Instant::now();
    let stats = compress_adaptive_to_file(&input, &pack_path)?;
    let compression_s = t0.elapsed().as_secs_f64();
    let rss_after_compress = read_status_kib("VmRSS:");

    let t1 = Instant::now();
    let output = decompress_file(&pack_path)?;
    let decompression_s = t1.elapsed().as_secs_f64();
    let rss_after_decompress = read_status_kib("VmRSS:");
    let peak_rss_hwm_kib = read_status_kib("VmHWM:");

    let output_valid = input == output;

    let ratio = if stats.original_size > 0 {
        stats.compressed_size as f64 / stats.original_size as f64
    } else {
        0.0
    };
    let savings_percent = (1.0 - ratio) * 100.0;

    let mib = stats.original_size as f64 / (1024.0 * 1024.0);
    let compression_mibs = if compression_s > 0.0 {
        mib / compression_s
    } else {
        0.0
    };
    let decompression_mibs = if decompression_s > 0.0 {
        mib / decompression_s
    } else {
        0.0
    };

    let now = format_timestamp_local(SystemTime::now());
    let skipped_candidates = if stats.skipped_candidates.is_empty() {
        "- none".to_string()
    } else {
        stats
            .skipped_candidates
            .iter()
            .map(|entry| format!("- {entry}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let report = format!(
        "zBit-rs Real File Benchmark Report\n\
Generated: {now}\n\
Input file: {input_path}\n\
Compressed artifact: {pack_path}\n\
\n\
Active profile: {active_profile}\n\
Selected method: {method}\n\
Selection reason: {reason}\n\
Circuit evaluation rule: {rule}\n\
\n\
Skipped candidates:\n\
{skipped_candidates}\n\
\n\
Raw candidate size (bytes): {raw_candidate}\n\
Indexed-raw candidate size (bytes): {indexed_raw_candidate}\n\
Indexed-circuit candidate size (bytes): {indexed_circuit}\n\
Indexed-huffman candidate size (bytes): {indexed_huffman}\n\
Raw-deflate candidate size (bytes): {raw_deflate}\n\
Raw-zstd candidate size (bytes): {raw_zstd}\n\
Raw-brotli candidate size (bytes): {raw_brotli}\n\
Raw-xz candidate size (bytes): {raw_xz}\n\
Framed-raw candidate size (bytes): {framed_raw}\n\
Recursive-circuit-xz candidate size (bytes): {recursive_circuit_xz}\n\
Monotonic-delta candidate size (bytes): {monotonic_delta}\n\
Prime-sequence candidate size (bytes): {prime_sequence}\n\
Adaptive-transformed-xz candidate size (bytes): {adaptive_transformed_xz}\n\
Exact-block-atlas candidate size (bytes): {exact_block_atlas}\n\
ZIP tensor split candidate size (bytes): {zip_tensor_split}\n\
\n\
Original size (bytes): {orig}\n\
Compressed size (bytes): {comp}\n\
Compression ratio (compressed/original): {ratio:.6}\n\
Space savings (%): {savings:.2}\n\
\n\
Compression time (ms): {comp_ms:.3}\n\
Decompression time (ms): {decomp_ms:.3}\n\
Compression throughput (MiB/s): {comp_mibs:.3}\n\
Decompression throughput (MiB/s): {decomp_mibs:.3}\n\
\n\
Candidate timing breakdown (ms):\n\
- index stream build: {index_stream_ms:.3}\n\
- huffman stream build: {huffman_stream_ms:.3}\n\
- raw-deflate encode: {raw_deflate_ms:.3}\n\
- raw-zstd encode: {raw_zstd_ms:.3}\n\
- raw-brotli gate/encode: {raw_brotli_ms:.3}\n\
- raw-xz encode: {raw_xz_ms:.3}\n\
- framed extraction: {framed_extraction_ms:.3}\n\
- recursive preflate search: {recursive_preflate_ms:.3}\n\
- recursive transform sampling: {recursive_transform_sampling_ms:.3}\n\
- recursive transform evaluation: {recursive_transform_eval_ms:.3}\n\
- recursive correction modeling: {recursive_correction_modeling_ms:.3}\n\
- recursive total: {recursive_total_ms:.3}\n\
- candidate validation: {candidate_validation_ms:.3}\n\
\n\
Cache stats:\n\
- codec hits/misses: {codec_hits}/{codec_misses}\n\
- tuned-xz hits/misses: {tuned_xz_hits}/{tuned_xz_misses}\n\
- preflate hits/misses: {preflate_hits}/{preflate_misses}\n\
- stream-range hits/misses: {stream_range_hits}/{stream_range_misses}\n\
\n\
Resource usage (KiB):\n\
- RSS before compression: {rss_before}\n\
- RSS after compression: {rss_after_comp}\n\
- RSS after decompression: {rss_after_decomp}\n\
- Compression RSS delta: {rss_delta_comp}\n\
- Decompression RSS delta: {rss_delta_decomp}\n\
- Peak RSS (VmHWM): {rss_hwm}\n\
\n\
Unique symbols: {unique}\n\
Bits per symbol index: {bits}\n\
Raw dictionary bytes: {raw_dict}\n\
Circuit dictionary bytes: {circuit_dict}\n\
Huffman dictionary bytes: {huffman_dict}\n\
Packed index payload bytes: {payload}\n\
\n\
Output validation: {valid}\n",
        active_profile = stats.active_profile,
        method = stats.chosen_method.name(),
        reason = stats.chosen_reason,
        rule = stats.circuit_rule_note,
        skipped_candidates = skipped_candidates,
        raw_candidate = stats.raw_candidate_bytes,
        indexed_raw_candidate = stats.indexed_raw_candidate_bytes,
        indexed_circuit = stats
            .indexed_circuit_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "skipped by rules".to_string()),
        indexed_huffman = stats
            .indexed_huffman_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        raw_deflate = stats
            .raw_deflate_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        raw_zstd = stats
            .raw_zstd_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        raw_brotli = stats
            .raw_brotli_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        raw_xz = stats
            .raw_xz_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        framed_raw = stats
            .framed_raw_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        recursive_circuit_xz = stats
            .recursive_circuit_xz_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        monotonic_delta = stats
            .monotonic_delta_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        prime_sequence = stats
            .prime_sequence_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        adaptive_transformed_xz = stats
            .adaptive_transformed_xz_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        exact_block_atlas = stats
            .exact_block_atlas_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        zip_tensor_split = stats
            .zip_tensor_split_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "unavailable".to_string()),
        orig = stats.original_size,
        comp = stats.compressed_size,
        ratio = ratio,
        savings = savings_percent,
        comp_ms = compression_s * 1000.0,
        decomp_ms = decompression_s * 1000.0,
        comp_mibs = compression_mibs,
        decomp_mibs = decompression_mibs,
        index_stream_ms = stats.timings.index_stream_ms,
        huffman_stream_ms = stats.timings.huffman_stream_ms,
        raw_deflate_ms = stats.timings.raw_deflate_ms,
        raw_zstd_ms = stats.timings.raw_zstd_ms,
        raw_brotli_ms = stats.timings.raw_brotli_ms,
        raw_xz_ms = stats.timings.raw_xz_ms,
        framed_extraction_ms = stats.timings.framed_extraction_ms,
        recursive_preflate_ms = stats.timings.recursive_preflate_ms,
        recursive_transform_sampling_ms = stats.timings.recursive_transform_sampling_ms,
        recursive_transform_eval_ms = stats.timings.recursive_transform_eval_ms,
        recursive_correction_modeling_ms = stats.timings.recursive_correction_modeling_ms,
        recursive_total_ms = stats.timings.recursive_total_ms,
        candidate_validation_ms = stats.timings.candidate_validation_ms,
        codec_hits = stats.cache_stats.codec_hits,
        tuned_xz_hits = stats.cache_stats.tuned_xz_hits,
        tuned_xz_misses = stats.cache_stats.tuned_xz_misses,
        codec_misses = stats.cache_stats.codec_misses,
        preflate_hits = stats.cache_stats.preflate_hits,
        preflate_misses = stats.cache_stats.preflate_misses,
        stream_range_hits = stats.cache_stats.stream_range_hits,
        stream_range_misses = stats.cache_stats.stream_range_misses,
        rss_before = format_opt_u64(rss_before_compress),
        rss_after_comp = format_opt_u64(rss_after_compress),
        rss_after_decomp = format_opt_u64(rss_after_decompress),
        rss_delta_comp = format_opt_delta(rss_before_compress, rss_after_compress),
        rss_delta_decomp = format_opt_delta(rss_after_compress, rss_after_decompress),
        rss_hwm = format_opt_u64(peak_rss_hwm_kib),
        unique = stats.unique_symbols,
        bits = stats.bits_per_symbol,
        raw_dict = stats.raw_dictionary_bytes,
        circuit_dict = stats.circuit_dictionary_bytes,
        huffman_dict = stats.huffman_dictionary_bytes,
        payload = stats.payload_bytes,
        valid = if output_valid { "PASS" } else { "FAIL" },
    );

    fs::write(&report_path, report.as_bytes())?;

    println!("benchmark input: {input_path}");
    println!("compressed artifact: {pack_path}");
    println!("report file: {report_path}");
    println!("selected method: {}", stats.chosen_method.name());
    println!("selection reason: {}", stats.chosen_reason);
    println!("original bytes: {}", stats.original_size);
    println!("compressed bytes: {}", stats.compressed_size);
    println!(
        "output validation: {}",
        if output_valid { "PASS" } else { "FAIL" }
    );

    if !output_valid {
        return Err("decompressed output does not match input".into());
    }

    Ok(())
}
