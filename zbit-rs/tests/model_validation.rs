// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::time::{SystemTime, UNIX_EPOCH};

use zbit_rs::ZbitModel;

#[test]
fn xor_truth_table_roundtrip() {
    let outputs = [0u8, 1, 1, 0];

    let mut model = ZbitModel::new(2).expect("new model");
    model
        .compress_from_table(&outputs, None)
        .expect("compress xor table");

    model
        .validate_against_table(&outputs)
        .expect("validate xor table");

    let stats = model.stats();
    assert_eq!(stats.implicant_count, 2);
    assert_eq!(stats.literal_count, 4);
}

#[test]
fn dont_care_truth_table_roundtrip() {
    let outputs = [0u8, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];
    let dont_cares = [1u8, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    let mut model = ZbitModel::new(4).expect("new model");
    model
        .compress_from_table(&outputs, Some(&dont_cares))
        .expect("compress with dc");

    let decoded = model.decompress_to_table().expect("decompress table");
    for idx in 0..outputs.len() {
        if dont_cares[idx] != 0 {
            continue;
        }
        assert_eq!(decoded[idx], outputs[idx]);
    }
}

#[test]
fn save_and_load_model_file() {
    let outputs = [1u8, 1, 0, 1, 1, 1, 0, 0];
    let mut model = ZbitModel::new(3).expect("new model");
    model
        .compress_from_table(&outputs, None)
        .expect("compress table");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("zbit_model_validation_{stamp}.zbit"));

    model.save_to_path(&path).expect("save model");
    let loaded = ZbitModel::load_from_path(&path).expect("load model");
    std::fs::remove_file(&path).ok();

    loaded
        .validate_against_table(&outputs)
        .expect("validate loaded model");
}
