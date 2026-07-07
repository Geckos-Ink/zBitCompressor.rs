// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use zbit_rs::{ZbitModel, ZbitResult};

fn validate_case(
    case_name: &str,
    num_inputs: u32,
    outputs: &[u8],
    dont_cares: Option<&[u8]>,
    roundtrip_to_file: bool,
) -> ZbitResult<()> {
    let mut model = ZbitModel::new(num_inputs)?;
    model.compress_from_table(outputs, dont_cares)?;

    let decoded = model.decompress_to_table()?;
    for idx in 0..outputs.len() {
        if dont_cares.map(|dc| dc[idx] != 0).unwrap_or(false) {
            continue;
        }

        let expected = if outputs[idx] == 0 { 0 } else { 1 };
        let actual = if decoded[idx] == 0 { 0 } else { 1 };
        if expected != actual {
            return Err(zbit_rs::ZbitError::ValidationMismatch {
                index: idx,
                expected,
                actual,
            });
        }
    }

    let stats = model.stats();
    println!(
        "[{case_name}] compression validated: gates={} (PIN={} NOT={} AND={} OR={} XOR={}), implicants={} literals={}",
        stats.node_count,
        stats.pin_count,
        stats.not_count,
        stats.and_count,
        stats.or_count,
        stats.xor_count,
        stats.implicant_count,
        stats.literal_count
    );

    if roundtrip_to_file {
        let file_name = format!("{case_name}.zbit");
        model.save_to_path(&file_name)?;
        let loaded = ZbitModel::load_from_path(&file_name)?;
        std::fs::remove_file(&file_name).ok();

        let redecoded = loaded.decompress_to_table()?;
        for idx in 0..outputs.len() {
            if dont_cares.map(|dc| dc[idx] != 0).unwrap_or(false) {
                continue;
            }

            let expected = if outputs[idx] == 0 { 0 } else { 1 };
            let actual = if redecoded[idx] == 0 { 0 } else { 1 };
            if expected != actual {
                return Err(zbit_rs::ZbitError::ValidationMismatch {
                    index: idx,
                    expected,
                    actual,
                });
            }
        }

        println!("[{case_name}] file roundtrip validated via {file_name}");
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let xor_outputs = [0u8, 1, 1, 0];

    let f3_outputs = [
        1u8, 1, 0, 1, 1, 1, 0, 0, // ON-set = {0,1,3,4,5}
    ];

    let f4_outputs = [0u8, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1];

    let f4_dont_cares = [1u8, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    validate_case("xor2", 2, &xor_outputs, None, true)?;
    validate_case("f3_exact", 3, &f3_outputs, None, true)?;
    validate_case("f4_with_dc", 4, &f4_outputs, Some(&f4_dont_cares), true)?;

    println!("All zbit-rs compression/decompression validations passed.");
    Ok(())
}
