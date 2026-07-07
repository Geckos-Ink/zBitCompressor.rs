// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[test]
#[ignore = "downloads a large asset and runs long multilevel stream benchmarks"]
fn cat_challenge_stream_multilevel_script_generates_valid_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = root.join("zbit-rs/scripts/benchmark_cat_challenge_stream_multilevel.sh");

    let status = std::process::Command::new("bash")
        .arg(script)
        .status()
        .expect("run cat challenge stream multilevel script");
    assert!(
        status.success(),
        "cat challenge stream multilevel benchmark script failed"
    );

    let report_path = root.join("zbit-rs/benchmark_cat_challenge_stream_multilevel_latest.txt");
    let report =
        std::fs::read_to_string(report_path).expect("read cat challenge stream multilevel report");

    assert!(
        report.contains("| Profile | Ratio |"),
        "stream multilevel report should include the summary table"
    );
    assert!(
        report.contains("| wide-overfit |"),
        "stream multilevel report should include wide-overfit profile"
    );
}
