// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

// Integration smoke test for the depth_anything benchmark script. Ignored by default
// because it downloads a multi-hundred-MB PyTorch model asset and runs a long benchmark.

#[test]
#[ignore = "downloads the depth_anything_v2_vits.pth asset and runs a long benchmark"]
fn depth_anything_script_generates_valid_report() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let script = root.join("zbit-rs/scripts/benchmark_depth_anything.sh");

    let status = std::process::Command::new("bash")
        .arg(script)
        .status()
        .expect("run depth_anything script");
    assert!(status.success(), "depth_anything benchmark script failed");

    let report_path = root.join("zbit-rs/benchmark_depth_anything_latest.txt");
    let report = std::fs::read_to_string(report_path).expect("read depth_anything report");

    assert!(
        report.contains("Output validation: PASS"),
        "depth_anything benchmark report should contain PASS validation"
    );
}
