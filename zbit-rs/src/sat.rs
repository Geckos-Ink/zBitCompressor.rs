// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[derive(Debug, Clone)]
pub struct Cnf {
    pub num_vars: usize,
    pub clauses: Vec<Vec<i32>>,
}

impl Cnf {
    pub fn new(num_vars: usize) -> Self {
        Self {
            num_vars,
            clauses: Vec::new(),
        }
    }

    pub fn push_clause(&mut self, clause: Vec<i32>) {
        self.clauses.push(clause);
    }
}

fn eval_lit(lit: i32, assignment: &[i8]) -> Option<bool> {
    let var = lit.unsigned_abs() as usize;
    let slot = assignment.get(var)?;

    match *slot {
        1 => Some(lit > 0),
        -1 => Some(lit < 0),
        _ => None,
    }
}

fn assign_lit(assignment: &mut [i8], lit: i32) -> bool {
    let var = lit.unsigned_abs() as usize;
    if var >= assignment.len() {
        return false;
    }

    let wanted = if lit > 0 { 1 } else { -1 };
    if assignment[var] == 0 {
        assignment[var] = wanted;
        true
    } else {
        assignment[var] == wanted
    }
}

fn unit_propagate(clauses: &[Vec<i32>], assignment: &mut [i8]) -> bool {
    loop {
        let mut changed = false;

        for clause in clauses {
            let mut satisfied = false;
            let mut unassigned_count = 0usize;
            let mut last_unassigned = 0i32;

            for &lit in clause {
                match eval_lit(lit, assignment) {
                    Some(true) => {
                        satisfied = true;
                        break;
                    }
                    Some(false) => {}
                    None => {
                        unassigned_count += 1;
                        last_unassigned = lit;
                    }
                }
            }

            if satisfied {
                continue;
            }

            if unassigned_count == 0 {
                return false;
            }

            if unassigned_count == 1 {
                if !assign_lit(assignment, last_unassigned) {
                    return false;
                }
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    true
}

fn choose_unassigned_var(num_vars: usize, assignment: &[i8]) -> Option<usize> {
    (1..=num_vars).find(|&var| assignment[var] == 0)
}

fn dpll(num_vars: usize, clauses: &[Vec<i32>], assignment: &mut [i8]) -> bool {
    if !unit_propagate(clauses, assignment) {
        return false;
    }

    if let Some(var) = choose_unassigned_var(num_vars, assignment) {
        let mut try_true = assignment.to_vec();
        try_true[var] = 1;
        if dpll(num_vars, clauses, &mut try_true) {
            return true;
        }

        let mut try_false = assignment.to_vec();
        try_false[var] = -1;
        return dpll(num_vars, clauses, &mut try_false);
    }

    true
}

pub fn is_satisfiable(cnf: &Cnf) -> bool {
    if cnf.num_vars == 0 {
        return cnf.clauses.iter().all(|c| !c.is_empty());
    }

    let mut assignment = vec![0i8; cnf.num_vars + 1];
    dpll(cnf.num_vars, &cnf.clauses, &mut assignment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sat_detects_simple_satisfiable_instance() {
        let mut cnf = Cnf::new(2);
        cnf.push_clause(vec![1, 2]);
        cnf.push_clause(vec![-1, 2]);

        assert!(is_satisfiable(&cnf));
    }

    #[test]
    fn sat_detects_unsat_instance() {
        let mut cnf = Cnf::new(1);
        cnf.push_clause(vec![1]);
        cnf.push_clause(vec![-1]);

        assert!(!is_satisfiable(&cnf));
    }
}
