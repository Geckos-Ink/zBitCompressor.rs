// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

// utils.rs
use std::mem;

/// Returns the bits of an integer from least significant to most significant.
pub fn get_bits(n: u32) -> Vec<u8> {
    if n == 0 {
        return vec![0];
    }
    let mut bits = Vec::new();
    let mut num = n;
    while num > 0 {
        bits.push((num % 2) as u8);
        num /= 2;
    }
    bits
}

/// Converts a float to its raw bit representation as a string.
pub fn float_to_bits(f: f32) -> String {
    let bytes: [u8; 4] = unsafe { mem::transmute(f.to_be()) }; // Big-endian
    bytes.iter().map(|b| format!("{:08b}", b)).collect()
}
