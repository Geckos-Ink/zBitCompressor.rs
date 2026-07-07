// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[test]
#[ignore = "downloads a 40MB asset and runs a long benchmark"]
fn cat_challenge_script_generates_valid_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = root.join("zbit-rs/scripts/benchmark_cat_challenge.sh");

    let status = std::process::Command::new("bash")
        .arg(script)
        .status()
        .expect("run cat challenge script");
    assert!(status.success(), "cat challenge benchmark script failed");

    let report_path = root.join("zbit-rs/benchmark_cat_challenge_latest.txt");
    let report = std::fs::read_to_string(report_path).expect("read cat challenge report");

    assert!(
        report.contains("Output validation: PASS"),
        "cat challenge benchmark report should contain PASS validation"
    );
}
