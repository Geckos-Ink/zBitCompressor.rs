// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use crate::error::{ZbitError, ZbitResult};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Implicant {
    pub value: u32,
    pub mask: u32,
}

impl Implicant {
    pub fn covers(self, minterm: u32) -> bool {
        (minterm & self.mask) == self.value
    }

    pub fn literal_count(self) -> u32 {
        self.mask.count_ones()
    }
}

fn dedup_implicants(values: &mut Vec<Implicant>) {
    values.sort_unstable_by_key(|i| (i.mask, i.value));
    values.dedup();
}

fn generate_prime_implicants(terms: &[u32], num_inputs: u32) -> Vec<Implicant> {
    if terms.is_empty() {
        return Vec::new();
    }

    let full_mask = if num_inputs == 0 {
        0
    } else {
        (1u32 << num_inputs) - 1
    };

    let mut current = terms
        .iter()
        .map(|&m| Implicant {
            value: m & full_mask,
            mask: full_mask,
        })
        .collect::<Vec<_>>();
    dedup_implicants(&mut current);

    let mut primes = Vec::new();

    while !current.is_empty() {
        let mut combined = vec![false; current.len()];
        let mut next = Vec::new();

        for i in 0..current.len() {
            for j in (i + 1)..current.len() {
                let a = current[i];
                let b = current[j];

                if a.mask != b.mask {
                    continue;
                }

                let diff = (a.value ^ b.value) & a.mask;
                if diff == 0 || !diff.is_power_of_two() {
                    continue;
                }

                combined[i] = true;
                combined[j] = true;

                next.push(Implicant {
                    value: a.value & !diff,
                    mask: a.mask & !diff,
                });
            }
        }

        for (idx, term) in current.iter().enumerate() {
            if !combined[idx] {
                primes.push(*term);
            }
        }

        dedup_implicants(&mut next);
        current = next;
    }

    dedup_implicants(&mut primes);
    primes
}

fn set_bit(bits: &mut [u64], idx: usize) {
    bits[idx >> 6] |= 1u64 << (idx & 63);
}

fn test_bit(bits: &[u64], idx: usize) -> bool {
    (bits[idx >> 6] & (1u64 << (idx & 63))) != 0
}

fn or_bits(dst: &mut [u64], src: &[u64]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d |= *s;
    }
}

fn bitset_full(bits: &[u64], nbits: usize) -> bool {
    let full_words = nbits / 64;
    let rem = nbits % 64;

    if bits.iter().take(full_words).any(|&word| word != u64::MAX) {
        return false;
    }

    if rem == 0 {
        return true;
    }

    let mask = (1u64 << rem) - 1;
    (bits[full_words] & mask) == mask
}

fn uncovered_count(covered: &[u64], nbits: usize) -> usize {
    let words = (nbits + 63) / 64;
    let mut total = 0usize;

    for i in 0..words {
        let full_mask = if i == words - 1 && (nbits % 64) != 0 {
            (1u64 << (nbits % 64)) - 1
        } else {
            u64::MAX
        };

        total += ((!covered[i]) & full_mask).count_ones() as usize;
    }

    total
}

fn branch_column(
    rows: &[Vec<u64>],
    covered: &[u64],
    selected: &[bool],
    on_count: usize,
) -> Option<usize> {
    let mut best_col = None;
    let mut best_options = usize::MAX;

    for col in 0..on_count {
        if test_bit(covered, col) {
            continue;
        }

        let mut options = 0usize;
        for (row_idx, row) in rows.iter().enumerate() {
            if selected[row_idx] {
                continue;
            }
            if test_bit(row, col) {
                options += 1;
            }
        }

        if options == 0 {
            return None;
        }

        if options < best_options {
            best_options = options;
            best_col = Some(col);
            if options == 1 {
                break;
            }
        }
    }

    best_col
}

fn new_gain(covered: &[u64], row: &[u64]) -> usize {
    covered
        .iter()
        .zip(row.iter())
        .map(|(c, r)| (r & !c).count_ones() as usize)
        .sum()
}

#[derive(Debug, Clone)]
struct CoverBest {
    terms: i32,
    literals: i32,
    selected: Vec<bool>,
}

fn dfs_cover(
    rows: &[Vec<u64>],
    literal_cost: &[u32],
    on_count: usize,
    covered: &[u64],
    selected: &mut [bool],
    selected_terms: i32,
    selected_literals: i32,
    best: &mut CoverBest,
) {
    if selected_terms > best.terms
        || (selected_terms == best.terms && selected_literals >= best.literals)
    {
        return;
    }

    if bitset_full(covered, on_count) {
        best.terms = selected_terms;
        best.literals = selected_literals;
        best.selected.copy_from_slice(selected);
        return;
    }

    let Some(col) = branch_column(rows, covered, selected, on_count) else {
        return;
    };

    let uncovered = uncovered_count(covered, on_count);
    let mut max_gain = 0usize;
    let mut min_lit = u32::MAX;

    for (idx, row) in rows.iter().enumerate() {
        if selected[idx] {
            continue;
        }

        let gain = new_gain(covered, row);
        if gain > max_gain {
            max_gain = gain;
        }
        if gain > 0 && literal_cost[idx] < min_lit {
            min_lit = literal_cost[idx];
        }
    }

    if max_gain == 0 {
        return;
    }

    let lb_terms = ((uncovered + max_gain - 1) / max_gain) as i32;
    if selected_terms + lb_terms > best.terms {
        return;
    }
    if selected_terms + lb_terms == best.terms
        && min_lit != u32::MAX
        && selected_literals + (min_lit as i32) * lb_terms >= best.literals
    {
        return;
    }

    let mut candidates = rows
        .iter()
        .enumerate()
        .filter(|(idx, row)| !selected[*idx] && test_bit(row, col))
        .map(|(idx, row)| (idx, new_gain(covered, row), literal_cost[idx]))
        .collect::<Vec<_>>();

    candidates.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));

    for (idx, _, lit_cost) in candidates {
        selected[idx] = true;

        let mut next_covered = covered.to_vec();
        or_bits(&mut next_covered, &rows[idx]);

        dfs_cover(
            rows,
            literal_cost,
            on_count,
            &next_covered,
            selected,
            selected_terms + 1,
            selected_literals + lit_cost as i32,
            best,
        );

        selected[idx] = false;
    }
}

fn select_minimum_cover(primes: &[Implicant], on_set: &[u32]) -> ZbitResult<Vec<Implicant>> {
    if on_set.is_empty() {
        return Ok(Vec::new());
    }
    if primes.is_empty() {
        return Err(ZbitError::Internal(
            "non-empty ON-set without prime implicants".to_string(),
        ));
    }

    let usable = primes
        .iter()
        .copied()
        .filter(|pi| on_set.iter().any(|&m| pi.covers(m)))
        .collect::<Vec<_>>();

    if usable.is_empty() {
        return Err(ZbitError::Internal(
            "prime implicants do not cover ON-set".to_string(),
        ));
    }

    let on_count = on_set.len();
    let words = (on_count + 63) / 64;

    let rows = usable
        .iter()
        .map(|pi| {
            let mut row = vec![0u64; words];
            for (col, &m) in on_set.iter().enumerate() {
                if pi.covers(m) {
                    set_bit(&mut row, col);
                }
            }
            row
        })
        .collect::<Vec<_>>();

    let literal_cost = usable.iter().map(|i| i.literal_count()).collect::<Vec<_>>();

    let mut selected = vec![false; usable.len()];
    let mut covered = vec![0u64; words];
    let mut base_terms = 0i32;
    let mut base_literals = 0i32;

    let mut changed = true;
    while changed {
        changed = false;

        for col in 0..on_count {
            if test_bit(&covered, col) {
                continue;
            }

            let mut unique_row = None;
            let mut count = 0usize;

            for (idx, row) in rows.iter().enumerate() {
                if selected[idx] {
                    continue;
                }
                if test_bit(row, col) {
                    count += 1;
                    unique_row = Some(idx);
                    if count > 1 {
                        break;
                    }
                }
            }

            match (count, unique_row) {
                (0, _) => {
                    return Err(ZbitError::Internal(
                        "ON-set column left uncovered".to_string(),
                    ))
                }
                (1, Some(idx)) if !selected[idx] => {
                    selected[idx] = true;
                    base_terms += 1;
                    base_literals += literal_cost[idx] as i32;
                    or_bits(&mut covered, &rows[idx]);
                    changed = true;
                }
                _ => {}
            }
        }
    }

    let mut best = CoverBest {
        terms: i32::MAX,
        literals: i32::MAX,
        selected: vec![false; usable.len()],
    };

    if bitset_full(&covered, on_count) {
        best.terms = base_terms;
        best.literals = base_literals;
        best.selected.copy_from_slice(&selected);
    } else {
        dfs_cover(
            &rows,
            &literal_cost,
            on_count,
            &covered,
            &mut selected,
            base_terms,
            base_literals,
            &mut best,
        );
    }

    if best.terms == i32::MAX {
        return Err(ZbitError::Internal(
            "failed to find exact minimum cover".to_string(),
        ));
    }

    let selected_implicants = usable
        .iter()
        .zip(best.selected.iter())
        .filter_map(|(imp, selected_flag)| if *selected_flag { Some(*imp) } else { None })
        .collect::<Vec<_>>();

    Ok(selected_implicants)
}

pub fn minimize_exact(
    num_inputs: u32,
    on_set: &[u32],
    dc_set: &[u32],
) -> ZbitResult<(Vec<Implicant>, u32)> {
    if on_set.is_empty() {
        return Ok((Vec::new(), 0));
    }

    let mut terms = Vec::with_capacity(on_set.len() + dc_set.len());
    terms.extend(on_set.iter().copied());
    terms.extend(dc_set.iter().copied());
    terms.sort_unstable();
    terms.dedup();

    let primes = generate_prime_implicants(&terms, num_inputs);
    let cover = select_minimum_cover(&primes, on_set)?;

    let literal_count = cover.iter().map(|i| i.literal_count()).sum();
    Ok((cover, literal_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_has_two_implicants() {
        let on = vec![1u32, 2u32];
        let (cover, literals) = minimize_exact(2, &on, &[]).expect("minimize exact");
        assert_eq!(cover.len(), 2);
        assert_eq!(literals, 4);
    }

    #[test]
    fn dont_care_can_reduce_literals() {
        let on = vec![1u32, 3u32];
        let dc = vec![0u32, 2u32];
        let (_cover, literals) = minimize_exact(2, &on, &dc).expect("minimize exact");
        assert!(literals <= 1);
    }
}
