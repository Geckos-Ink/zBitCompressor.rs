// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[test]
#[ignore = "downloads a 40MB asset and runs a long stream benchmark"]
fn cat_challenge_stream_script_generates_valid_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = root.join("zbit-rs/scripts/benchmark_cat_challenge_stream.sh");

    let status = std::process::Command::new("bash")
        .arg(script)
        .status()
        .expect("run cat challenge stream script");
    assert!(
        status.success(),
        "cat challenge stream benchmark script failed"
    );

    let report_path = root.join("zbit-rs/benchmark_cat_challenge_stream_latest.txt");
    let report = std::fs::read_to_string(report_path).expect("read cat challenge stream report");

    assert!(
        report.contains("Output validation: PASS"),
        "cat challenge stream report should contain PASS output validation"
    );
    assert!(
        report.contains("Key-piece resume validation: PASS"),
        "cat challenge stream report should contain PASS key-piece resume validation"
    );
}
