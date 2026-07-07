// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use zbit_rs::{AdvancedOptions, MappingObjective, ZbitModel};

#[test]
fn advanced_flow_validates_truth_table_when_forcing_heuristic_seed() {
    let outputs = [
        0u8, 1, 1, 0, 1, 1, 0, 0, // 3-input function
    ];

    let mut model = ZbitModel::new(3).expect("new model");
    let options = AdvancedOptions {
        exact_seed_max_inputs: 0,
        espresso_rounds: 6,
        objective: MappingObjective::AsicArea,
        ..AdvancedOptions::default()
    };

    let report = model
        .compress_from_table_advanced(&outputs, None, &options)
        .expect("advanced compress");

    model
        .validate_against_table(&outputs)
        .expect("advanced table validation");

    assert!(report.used_espresso);
    assert!(!report.used_exact_seed);
    assert!(report.selected.weighted_cost.is_finite());
}

#[test]
fn objective_specific_entrypoint_runs_for_fpga_model() {
    let outputs = [
        0u8, 0, 0, 1, 0, 1, 1, 1, // 3-input majority-like behavior
    ];

    let mut model = ZbitModel::new(3).expect("new model");
    let report = model
        .compress_from_table_with_objective(&outputs, None, MappingObjective::FpgaLut6)
        .expect("compress with objective");

    model
        .validate_against_table(&outputs)
        .expect("objective validation");

    assert!(report.selected.estimated_luts > 0);
    assert!(report.selected.literal_count > 0);
}
