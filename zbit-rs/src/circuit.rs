// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

// src/circuit.rs
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io;
use std::rc::Rc;
use crate::utils::{float_to_bits, get_bits};
use crate::file_format;
pub mod minimizer; // <-- Add minimizer module

// Type alias for a shared, mutable reference to a Gate
pub type GateRef = Rc<RefCell<Gate>>;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum GateType {
    Pin,
    Not,
    And,
    Or,
    Xor,
    Dff,
    Latch,
}

impl GateType {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(GateType::Pin),
            1 => Some(GateType::Not),
            2 => Some(GateType::And),
            3 => Some(GateType::Or),
            4 => Some(GateType::Xor),
            5 => Some(GateType::Dff),
            6 => Some(GateType::Latch),
            _ => None,
        }
    }
}

pub struct Gate {
    pub gate_type: GateType,
    pub value: i32, // Used as index for Pin
    pub args: Vec<GateRef>,
    pub clock: Option<u32>,
    hash_cache: RefCell<Option<u64>>,
}

// Manual implementation to control hashing logic
impl Hash for Gate {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(cached_hash) = *self.hash_cache.borrow() {
            state.write_u64(cached_hash);
            return;
        }

        (self.gate_type as u8).hash(state);
        if self.gate_type == GateType::Pin {
            self.value.hash(state);
        }

        let mut arg_hashes: Vec<u64> = self.args.iter().map(|arg| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            arg.borrow().hash(&mut hasher);
            hasher.finish()
        }).collect();

        if matches!(self.gate_type, GateType::And | GateType::Or | GateType::Xor) {
            arg_hashes.sort_unstable();
        }

        arg_hashes.hash(state);
        
        let final_hash = state.finish();
        *self.hash_cache.borrow_mut() = Some(final_hash);
    }
}

impl PartialEq for Gate {
    fn eq(&self, other: &Self) -> bool {
        if self.gate_type != other.gate_type { return false; }
        if self.gate_type == GateType::Pin && self.value != other.value { return false; }
        
        let mut hasher1 = std::collections::hash_map::DefaultHasher::new();
        self.hash(&mut hasher1);
        let mut hasher2 = std::collections::hash_map::DefaultHasher::new();
        other.hash(&mut hasher2);
        hasher1.finish() == hasher2.finish()
    }
}
impl Eq for Gate {}

impl Gate {
    pub fn new(gate_type: GateType, value: i32) -> GateRef {
        Rc::new(RefCell::new(Gate {
            gate_type,
            value,
            args: Vec::new(),
            clock: None,
            hash_cache: RefCell::new(None),
        }))
    }

    pub fn add_arg(&mut self, arg: &GateRef) {
        self.args.push(Rc::clone(arg));
        self.invalidate_hash();
    }
    
    fn invalidate_hash(&self) {
        *self.hash_cache.borrow_mut() = None;
    }

    pub fn evaluate(&self, inputs: &HashMap<u32, bool>) -> bool {
        match self.gate_type {
            GateType::Pin => *inputs.get(&(self.value as u32)).unwrap_or(&false),
            GateType::Not => !self.args[0].borrow().evaluate(inputs),
            GateType::And => self.args.iter().all(|g| g.borrow().evaluate(inputs)),
            GateType::Or => self.args.iter().any(|g| g.borrow().evaluate(inputs)),
            GateType::Xor => self.args.iter().fold(false, |acc, g| acc ^ g.borrow().evaluate(inputs)),
            _ => unimplemented!("DFF and Latch evaluation not implemented for this example"),
        }
    }
}

pub struct BitsMap {
    pins: Vec<GateRef>,
    gates: HashMap<u64, GateRef>,
    map: GateRef, // The main output gate
    minterms: HashSet<u32>, // <-- NEW: Store the ON-set minterms
    num_vars: usize,        // <-- NEW: Store the number of variables
}

impl BitsMap {
    pub fn new(num_inputs: usize) -> Self {
        let mut map = BitsMap {
            pins: Vec::new(),
            gates: HashMap::new(),
            map: Gate::new(GateType::Or, -1),
            minterms: HashSet::new(),
            num_vars: num_inputs,
        };
        map.check_gate(Rc::clone(&map.map)); // Register root
        for i in 0..num_inputs {
            map.add_pin(i as u32);
        }
        map
    }

    pub fn add_pin(&mut self, pin_index: u32) -> GateRef {
        if let Some(pin) = self.pins.iter().find(|p| p.borrow().value == pin_index as i32) {
            return Rc::clone(pin);
        }
        let pin_gate = Gate::new(GateType::Pin, pin_index as i32);
        let checked_pin = self.check_gate(pin_gate);
        self.pins.push(Rc::clone(&checked_pin));
        self.pins.sort_by_key(|p| p.borrow().value);
        checked_pin
    }
    
    pub fn get_pin(&self, pin_index: u32) -> Option<GateRef> {
        self.pins.iter()
            .find(|p| p.borrow().value == pin_index as i32)
            .map(Rc::clone)
    }

    pub fn check_gate(&mut self, gate: GateRef) -> GateRef {
        if gate.borrow().gate_type == GateType::Not && !gate.borrow().args.is_empty() {
             let inner_gate = &gate.borrow().args[0];
             if inner_gate.borrow().gate_type == GateType::Not && !inner_gate.borrow().args.is_empty(){
                 return Rc::clone(&inner_gate.borrow().args[0]);
             }
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        gate.borrow().hash(&mut hasher);
        let hash = hasher.finish();

        if let Some(existing_gate) = self.gates.get(&hash) {
            return Rc::clone(existing_gate);
        }

        self.gates.insert(hash, Rc::clone(&gate));
        gate
    }
    
    /// Collects minterms from the truth table.
    pub fn add_minterm(&mut self, series: &[u8], output_bit: u8) {
        if output_bit == 1 {
            let mut minterm_val = 0u32;
            for (i, &val) in series.iter().enumerate() {
                if val == 1 {
                    minterm_val |= 1 << i;
                }
            }
            self.minterms.insert(minterm_val);
        }
    }

    /// NEW: Optimizes the circuit using the Quine-McCluskey algorithm.
    pub fn quine_mccluskey_optimize(&mut self) {
        // Run the Q-M algorithm on the collected minterms
        let minimal_implicants = minimizer::quine_mccluskey(&self.minterms, self.num_vars);
        
        // Reset the root OR gate and clear its arguments
        self.map.borrow_mut().args.clear();
        self.map.borrow_mut().invalidate_hash();
        self.gates.clear(); // Clear all old gates
        self.check_gate(Rc::clone(&self.map));
        for pin in &self.pins { // Re-register pins
            self.check_gate(Rc::clone(pin));
        }

        if minimal_implicants.is_empty() && !self.minterms.is_empty() {
            // This means the function is always TRUE.
            // We can represent this with an AND gate with no inputs.
            let true_gate = self.check_gate(Gate::new(GateType::And, -1));
            self.map.borrow_mut().add_arg(&true_gate);
            return;
        }

        // Build the new, optimized circuit from the minimal implicants
        for implicant in &minimal_implicants {
            let and_gate_ref = Gate::new(GateType::And, -1);
            let mut has_literals = false;
            for i in 0..self.num_vars {
                if (implicant.mask >> i) & 1 == 1 { // Check if this variable is in the term
                    has_literals = true;
                    let pin_gate = self.get_pin(i as u32).expect("Pin should exist");
                    
                    if (implicant.value >> i) & 1 == 1 { // If bit is 1, use pin directly
                        and_gate_ref.borrow_mut().add_arg(&pin_gate);
                    } else { // If bit is 0, use NOT(pin)
                        let not_gate = Gate::new(GateType::Not, -1);
                        not_gate.borrow_mut().add_arg(&pin_gate);
                        let checked_not = self.check_gate(not_gate);
                        and_gate_ref.borrow_mut().add_arg(&checked_not);
                    }
                }
            }

            if has_literals {
                let final_and_gate = self.check_gate(and_gate_ref);
                self.map.borrow_mut().add_arg(&final_and_gate);
            }
        }
        self.final_compression(); // Perform final simple optimizations
    }
    
    // Creates a logic gate structure from a single row of a truth table
    pub fn set(&mut self, series: &[u8], output_bit: u8) {
        if output_bit == 0 {
            return; // Build Sum-of-Products, so only care about outputs of 1
        }
        
        let and_gate_ref = Gate::new(GateType::And, -1);
        
        for (i, &input_val) in series.iter().enumerate() {
            let pin_gate = self.get_pin(i as u32).expect("Pin should exist");
            let term_gate = if input_val == 0 {
                let not_gate = Gate::new(GateType::Not, -1);
                not_gate.borrow_mut().add_arg(&pin_gate);
                self.check_gate(not_gate)
            } else {
                pin_gate
            };
            and_gate_ref.borrow_mut().add_arg(&term_gate);
        }
        
        let final_and_gate = self.check_gate(and_gate_ref);
        self.map.borrow_mut().add_arg(&final_and_gate);
    }
    
    // Flattens nested AND/OR gates and removes duplicates
       fn optimize_gate(gate_ref: &GateRef, _map: &mut BitsMap) -> bool {
        let mut gate = gate_ref.borrow_mut();
        let gate_type = gate.gate_type;
        
        if !matches!(gate_type, GateType::And | GateType::Or | GateType::Xor) {
            return false;
        }
        
        let mut changed = false;
        let mut i = 0;
        while i < gate.args.len() {
            let arg_gate_type = gate.args[i].borrow().gate_type;
            if arg_gate_type == gate_type {
                let sub_args = gate.args.remove(i).borrow().args.clone();
                for sub_arg in sub_args {
                    gate.args.push(sub_arg);
                }
                changed = true;
            } else {
                i += 1;
            }
        }
        
        let mut seen_hashes = HashSet::new();
        let mut unique_args = Vec::new();
        for arg in gate.args.iter() {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            arg.borrow().hash(&mut hasher);
            if seen_hashes.insert(hasher.finish()) {
                unique_args.push(Rc::clone(arg));
            } else {
                changed = true;
            }
        }
        
        if changed {
            gate.args = unique_args;
            gate.invalidate_hash();
        }

        changed
    }

    pub fn final_compression(&mut self) {
        let all_gates: Vec<_> = self.gates.values().cloned().collect();
        for gate_ref in all_gates {
             if Self::optimize_gate(&gate_ref, self) {
                // Future enhancement: re-check gates if they change
             }
        }
        self.map = self.check_gate(Rc::clone(&self.map));
    }
    
    pub fn evaluate(&self, inputs: &Vec<bool>) -> bool {
        let input_map: HashMap<u32, bool> = inputs.iter().enumerate().map(|(i, &v)| (i as u32, v)).collect();
        self.map.borrow().evaluate(&input_map)
    }

    pub fn verify_logic(&self, truth_table: &[(Vec<bool>, bool)]) -> bool {
        for (inputs, expected) in truth_table {
            if self.evaluate(inputs) != *expected {
                println!("Verification FAILED for inputs {:?}: expected {}, got {}", inputs, expected, !*expected);
                return false;
            }
        }
        true
    }
    
    pub fn visualize(&self) {
        fn visualize_recursive(gate_ref: &GateRef, prefix: String, is_last: bool, visited: &mut HashSet<u64>) {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            gate_ref.borrow().hash(&mut hasher);
            let hash = hasher.finish();

            let connector = if is_last { "└── " } else { "├── " };
            let gate = gate_ref.borrow();
            let type_str = format!("{:?}", gate.gate_type);
            let display_val = if gate.gate_type == GateType::Pin { format!("({})", gate.value) } else { "".to_string() };
            println!("{}{}{}{}", prefix, connector, type_str, display_val);
            
            if !visited.insert(hash) {
                return;
            }

            let new_prefix = prefix + if is_last { "    " } else { "│   " };
            let arg_count = gate.args.len();
            for (i, arg) in gate.args.iter().enumerate() {
                visualize_recursive(arg, new_prefix.clone(), i == arg_count - 1, visited);
            }
        }
        let mut visited = HashSet::new();
        visualize_recursive(&self.map, "".to_string(), true, &mut visited);
    }
    
    pub fn get_map_hash(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.map.borrow().hash(&mut hasher);
        hasher.finish()
    }

    pub fn save(&self, path: &str) -> io::Result<()> {
        file_format::save_circuit(self, path)
    }

    pub fn load(path: &str) -> io::Result<Self> {
        file_format::load_circuit(path)
    }

    pub(crate) fn get_all_gates_flattened(&self) -> Vec<GateRef> {
        let mut all_gates = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        
        queue.push_back(Rc::clone(&self.map));
        
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.map.borrow().hash(&mut hasher);
        visited.insert(hasher.finish());

        while let Some(gate_ref) = queue.pop_front() {
            all_gates.push(Rc::clone(&gate_ref));
            for arg in gate_ref.borrow().args.iter() {
                 let mut hasher = std::collections::hash_map::DefaultHasher::new();
                 arg.borrow().hash(&mut hasher);
                 if visited.insert(hasher.finish()){
                    queue.push_back(Rc::clone(arg));
                 }
            }
        }
        all_gates
    }
    
    pub(crate) fn from_gates(root_gate: GateRef, all_gates_vec: Vec<GateRef>, num_vars: usize) -> Self {
        let mut gates = HashMap::new();
         for gate_ref in &all_gates_vec {
             let mut hasher = std::collections::hash_map::DefaultHasher::new();
             gate_ref.borrow().hash(&mut hasher);
             gates.insert(hasher.finish(), Rc::clone(gate_ref));
         }
        
         let mut pins: Vec<_> = gates.values()
            .filter(|g| g.borrow().gate_type == GateType::Pin)
            .cloned()
            .collect();
         pins.sort_by_key(|p| p.borrow().value);
        
        BitsMap {
            pins,
            gates,
            map: root_gate,
            minterms: HashSet::new(), // Minterms are not saved in the file
            num_vars,
        }
    }          
}
