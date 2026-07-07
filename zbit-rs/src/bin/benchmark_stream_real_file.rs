// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::env;
use std::fs;
use std::time::{Instant, SystemTime};

use zbit_rs::{
    compress_adaptive_stream_to_file, decompress_stream_file,
    decompress_stream_file_from_key_piece, StreamPackOptions,
};

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

fn parse_arg_usize(args: &[String], idx: usize, default_value: usize) -> usize {
    args.get(idx)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(default_value)
}

fn parse_arg_u8(args: &[String], idx: usize, default_value: u8) -> u8 {
    args.get(idx)
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(default_value)
}

fn parse_arg_bool(args: &[String], idx: usize, default_value: bool) -> bool {
    let Some(value) = args.get(idx) else {
        return default_value;
    };

    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default_value,
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
    let args: Vec<String> = env::args().collect();

    let input_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "../papers/zbit-algorithmsResearch.md".to_string());
    let pack_path = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| "benchmark_stream.zbps".to_string());
    let report_path = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "benchmark_stream_latest.txt".to_string());

    let chunk_size = parse_arg_usize(&args, 4, 256 * 1024);
    let key_piece_interval = parse_arg_usize(&args, 5, 8);
    let max_group_depth = parse_arg_u8(&args, 6, 2);
    let max_group_pieces = parse_arg_usize(&args, 7, 8);
    let realtime_mode = parse_arg_bool(&args, 8, true);
    let wide_overfitting_circuits = parse_arg_bool(&args, 9, true);
    let carry_grouping_history = parse_arg_bool(&args, 10, true);

    let options = StreamPackOptions {
        chunk_size,
        key_piece_interval,
        max_group_depth,
        max_group_pieces,
        carry_grouping_history,
        realtime_mode,
        wide_overfitting_circuits,
    };

    let input = fs::read(&input_path)?;
    let rss_before_compress = read_status_kib("VmRSS:");

    let t0 = Instant::now();
    let stats = compress_adaptive_stream_to_file(&input, &pack_path, &options)?;
    let compression_s = t0.elapsed().as_secs_f64();
    let rss_after_compress = read_status_kib("VmRSS:");

    let t1 = Instant::now();
    let output = decompress_stream_file(&pack_path)?;
    let decompression_s = t1.elapsed().as_secs_f64();
    let rss_after_decompress = read_status_kib("VmRSS:");
    let peak_rss_hwm_kib = read_status_kib("VmHWM:");

    let output_valid = input == output;

    let key_resume_start_chunk = if stats.total_chunks > stats.key_piece_interval {
        stats.key_piece_interval
    } else {
        0
    };

    let key_resume_validation = if key_resume_start_chunk > 0 {
        let resumed = decompress_stream_file_from_key_piece(&pack_path, key_resume_start_chunk)?;
        let start_offset = key_resume_start_chunk.saturating_mul(stats.chunk_size);
        resumed == input[start_offset..]
    } else {
        true
    };

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
        "zBit-rs Stream Benchmark Report\n\
Generated: {now}\n\
Input file: {input_path}\n\
Compressed artifact: {pack_path}\n\
\n\
Active profile: {active_profile}\n\
Streaming settings:\n\
- chunk size (bytes): {chunk_size}\n\
- key piece interval (chunks): {key_interval}\n\
- max group depth: {max_depth}\n\
- max group pieces: {max_group_pieces}\n\
- carry grouping history: {carry_grouping_history}\n\
- realtime mode: {realtime_mode}\n\
- requested wide overfitting circuits: {wide_overfitting_circuits}\n\
- effective wide overfitting circuits: {effective_wide_overfitting_circuits}\n\
- adaptive wide promotion used: {adaptive_wide_promotion_used}\n\
- shared grouping payload used: {shared_grouping_payload_used}\n\
\n\
Stream topology:\n\
- total chunks: {total_chunks}\n\
- key pieces (block starts): {key_pieces}\n\
- block count: {block_count}\n\
- piece nodes: {piece_nodes}\n\
- grouped nodes: {grouped_nodes}\n\
- split nodes: {split_nodes}\n\
- max depth used: {max_depth_used}\n\
- grouping hint updates: {grouping_hint_updates}\n\
- key-piece decode note: {key_note}\n\
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
- raw-deflate encode: {raw_deflate_ms:.3}\n\
- raw-zstd encode: {raw_zstd_ms:.3}\n\
- framed extraction: {framed_extraction_ms:.3}\n\
- recursive preflate search: {recursive_preflate_ms:.3}\n\
- recursive transform sampling: {recursive_transform_sampling_ms:.3}\n\
- recursive transform evaluation: {recursive_transform_eval_ms:.3}\n\
- recursive correction modeling: {recursive_correction_modeling_ms:.3}\n\
- recursive total: {recursive_total_ms:.3}\n\
- stream global/shared payload build: {stream_global_payload_ms:.3}\n\
- stream block planning: {stream_block_planning_ms:.3}\n\
- candidate validation: {candidate_validation_ms:.3}\n\
\n\
Cache stats:\n\
- codec hits/misses: {codec_hits}/{codec_misses}\n\
- preflate hits/misses: {preflate_hits}/{preflate_misses}\n\
- stream-range hits/misses: {stream_range_hits}/{stream_range_misses}\n\
\n\
Skipped candidates:\n\
{skipped_candidates}\n\
\n\
Resource usage (KiB):\n\
- RSS before compression: {rss_before}\n\
- RSS after compression: {rss_after_comp}\n\
- RSS after decompression: {rss_after_decomp}\n\
- Compression RSS delta: {rss_delta_comp}\n\
- Decompression RSS delta: {rss_delta_decomp}\n\
- Peak RSS (VmHWM): {rss_hwm}\n\
\n\
Output validation: {output_valid}\n\
Key-piece resume validation: {key_resume}\n",
        chunk_size = stats.chunk_size,
        active_profile = stats.active_profile,
        key_interval = stats.key_piece_interval,
        max_depth = stats.max_group_depth,
        max_group_pieces = stats.max_group_pieces,
        carry_grouping_history = options.carry_grouping_history,
        realtime_mode = options.realtime_mode,
        wide_overfitting_circuits = options.wide_overfitting_circuits,
        effective_wide_overfitting_circuits = stats.effective_wide_overfitting_circuits,
        adaptive_wide_promotion_used = stats.adaptive_wide_promotion_used,
        shared_grouping_payload_used = stats.shared_grouping_payload_used,
        total_chunks = stats.total_chunks,
        key_pieces = stats.key_piece_count,
        block_count = stats.block_count,
        piece_nodes = stats.piece_node_count,
        grouped_nodes = stats.grouped_node_count,
        split_nodes = stats.split_node_count,
        max_depth_used = stats.max_depth_used,
        grouping_hint_updates = stats.grouping_hint_updates,
        key_note = stats.key_piece_decode_note,
        orig = stats.original_size,
        comp = stats.compressed_size,
        ratio = ratio,
        savings = savings_percent,
        comp_ms = compression_s * 1000.0,
        decomp_ms = decompression_s * 1000.0,
        comp_mibs = compression_mibs,
        decomp_mibs = decompression_mibs,
        raw_deflate_ms = stats.timings.raw_deflate_ms,
        raw_zstd_ms = stats.timings.raw_zstd_ms,
        framed_extraction_ms = stats.timings.framed_extraction_ms,
        recursive_preflate_ms = stats.timings.recursive_preflate_ms,
        recursive_transform_sampling_ms = stats.timings.recursive_transform_sampling_ms,
        recursive_transform_eval_ms = stats.timings.recursive_transform_eval_ms,
        recursive_correction_modeling_ms = stats.timings.recursive_correction_modeling_ms,
        recursive_total_ms = stats.timings.recursive_total_ms,
        stream_global_payload_ms = stats.timings.stream_global_payload_ms,
        stream_block_planning_ms = stats.timings.stream_block_planning_ms,
        candidate_validation_ms = stats.timings.candidate_validation_ms,
        codec_hits = stats.cache_stats.codec_hits,
        codec_misses = stats.cache_stats.codec_misses,
        preflate_hits = stats.cache_stats.preflate_hits,
        preflate_misses = stats.cache_stats.preflate_misses,
        stream_range_hits = stats.cache_stats.stream_range_hits,
        stream_range_misses = stats.cache_stats.stream_range_misses,
        skipped_candidates = skipped_candidates,
        rss_before = format_opt_u64(rss_before_compress),
        rss_after_comp = format_opt_u64(rss_after_compress),
        rss_after_decomp = format_opt_u64(rss_after_decompress),
        rss_delta_comp = format_opt_delta(rss_before_compress, rss_after_compress),
        rss_delta_decomp = format_opt_delta(rss_after_compress, rss_after_decompress),
        rss_hwm = format_opt_u64(peak_rss_hwm_kib),
        output_valid = if output_valid { "PASS" } else { "FAIL" },
        key_resume = if key_resume_validation {
            "PASS"
        } else {
            "FAIL"
        },
    );

    fs::write(&report_path, report.as_bytes())?;

    println!("stream benchmark input: {input_path}");
    println!("compressed artifact: {pack_path}");
    println!("report file: {report_path}");
    println!(
        "stream settings: chunk={} bytes key_interval={} max_depth={} max_group_pieces={} realtime={} requested_wide_overfit={} effective_wide_overfit={} promotion_used={} shared_payload={} carry_history={}",
        stats.chunk_size,
        stats.key_piece_interval,
        stats.max_group_depth,
        stats.max_group_pieces,
        options.realtime_mode,
        options.wide_overfitting_circuits,
        stats.effective_wide_overfitting_circuits,
        stats.adaptive_wide_promotion_used,
        stats.shared_grouping_payload_used,
        options.carry_grouping_history
    );
    println!("original bytes: {}", stats.original_size);
    println!("compressed bytes: {}", stats.compressed_size);
    println!(
        "output validation: {}",
        if output_valid { "PASS" } else { "FAIL" }
    );
    println!(
        "key-piece resume validation: {}",
        if key_resume_validation {
            "PASS"
        } else {
            "FAIL"
        }
    );

    if !output_valid {
        return Err("stream decompressed output does not match input".into());
    }
    if !key_resume_validation {
        return Err("stream key-piece resume output does not match expected suffix".into());
    }

    Ok(())
}
