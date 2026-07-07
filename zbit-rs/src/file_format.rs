// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

// file_format.rs
use crate::circuit::{BitsMap, Gate, GateRef, GateType};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::rc::Rc;
use std::cell::RefCell;

const MAGIC_NUMBER: u32 = 0x5A424954; // "ZBIT"
const VERSION: u16 = 1;

/// Saves the circuit to a binary file.
/// Format:
/// - Magic Number (4 bytes)
/// - Version (2 bytes)
/// - Root Gate ID (4 bytes)
/// - Total Gate Count (4 bytes)
/// - Gates Data...
///   - Gate Type (1 byte)
///   - Gate Value (4 bytes, for Pins)
///   - Args Count (4 bytes)
///   - Arg IDs (4 bytes each)

pub fn save_circuit(map: &BitsMap, path: &str) -> io::Result<()> {
    let mut file = File::create(path)?;
    let all_gates = map.get_all_gates_flattened();
    let mut gate_to_id = HashMap::new();
    
    for (i, gate_ref) in all_gates.iter().enumerate() {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        gate_ref.borrow().hash(&mut hasher);
        gate_to_id.insert(hasher.finish(), i as u32);
    }
    
    file.write_all(&MAGIC_NUMBER.to_le_bytes())?;
    file.write_all(&VERSION.to_le_bytes())?;

    let root_hash = map.get_map_hash();
    let root_id = *gate_to_id.get(&root_hash).unwrap();
    file.write_all(&root_id.to_le_bytes())?;
    
    file.write_all(&(all_gates.len() as u32).to_le_bytes())?;

    for gate_ref in &all_gates {
        let gate = gate_ref.borrow();
        file.write_all(&(gate.gate_type as u8).to_le_bytes())?;

        if gate.gate_type == GateType::Pin {
            file.write_all(&gate.value.to_le_bytes())?;
        }
        
        file.write_all(&(gate.args.len() as u32).to_le_bytes())?;
        for arg in &gate.args {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            arg.borrow().hash(&mut hasher);
            let arg_id = *gate_to_id.get(&hasher.finish()).unwrap();
            file.write_all(&arg_id.to_le_bytes())?;
        }
    }
    
    Ok(())
}


pub fn load_circuit(path: &str) -> io::Result<BitsMap> {
    let mut file = File::open(path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let mut cursor = io::Cursor::new(buffer);

    let mut read_u32 = || -> io::Result<u32> {
        let mut bytes = [0; 4];
        cursor.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    };
    
    let magic = read_u32()?;
    if magic != MAGIC_NUMBER {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid magic number"));
    }
    
    let mut version_bytes = [0; 2];
    cursor.read_exact(&mut version_bytes)?;
    let _version = u16::from_le_bytes(version_bytes);

    let root_id = read_u32()? as usize;
    let gate_count = read_u32()? as usize;
    
    let mut gates_by_id: Vec<GateRef> = Vec::with_capacity(gate_count);
    let mut pin_count = 0;
    
    let original_pos = cursor.position();

    // First pass: create all gate objects without connections
    for _ in 0..gate_count {
        let gate_type_u8 = { let mut b = [0;1]; cursor.read_exact(&mut b)?; b[0] };
        let gate_type = GateType::from_u8(gate_type_u8)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid gate type"))?;
            
        let value = if gate_type == GateType::Pin {
            pin_count += 1;
            read_u32()? as i32
        } else {
            -1
        };
        
        let args_count = read_u32()?;
        cursor.set_position(cursor.position() + (args_count as u64 * 4)); // Skip args

        gates_by_id.push(Gate::new(gate_type, value));
    }
    
    // Second pass: connect the gates
    cursor.set_position(original_pos);
    
    for i in 0..gate_count {
        let _gate_type_u8 = { let mut b = [0;1]; cursor.read_exact(&mut b)?; b[0] };
        let gate_type = GateType::from_u8(_gate_type_u8).unwrap();
        
        if gate_type == GateType::Pin {
            let _ = read_u32()?;
        }
        
        let args_count = read_u32()? as usize;
        let mut gate = gates_by_id[i].borrow_mut();
        for _ in 0..args_count {
            let arg_id = read_u32()? as usize;
            let arg_gate_ref = Rc::clone(&gates_by_id[arg_id]);
            gate.add_arg(&arg_gate_ref);
        }
    }
    
    let root_gate = Rc::clone(&gates_by_id[root_id]);
    Ok(BitsMap::from_gates(root_gate, gates_by_id, pin_count))
}
