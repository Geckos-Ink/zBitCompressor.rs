// Licensed under the GNU Affero General Public License v3.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::advanced::{minimize_advanced, AdvancedOptions, AdvancedReport, MappingObjective};
use crate::error::{ZbitError, ZbitResult};
use crate::minimizer::{minimize_exact, Implicant};

pub const ZBIT_MAX_INPUTS_EXACT: u32 = 16;
const ZBIT_MAX_INPUTS_SUPPORTED: u32 = 31;
const ZBIT_MAGIC: u32 = 0x5A42_4954; // "ZBIT"
const ZBIT_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(u8)]
pub enum NodeType {
    Pin = 0,
    Not = 1,
    And = 2,
    Or = 3,
    Xor = 4,
}

impl NodeType {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Pin),
            1 => Some(Self::Not),
            2 => Some(Self::And),
            3 => Some(Self::Or),
            4 => Some(Self::Xor),
            _ => None,
        }
    }

    fn is_commutative(self) -> bool {
        matches!(self, Self::And | Self::Or | Self::Xor)
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub node_type: NodeType,
    pub value: i32,
    pub inputs: Vec<u32>,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct NodeKey {
    node_type: NodeType,
    value: i32,
    inputs: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct ZbitStats {
    pub num_inputs: u32,
    pub node_count: u32,
    pub pin_count: u32,
    pub not_count: u32,
    pub and_count: u32,
    pub or_count: u32,
    pub xor_count: u32,
    pub root_id: u32,
    pub implicant_count: u32,
    pub literal_count: u32,
}

#[derive(Debug, Clone)]
pub struct ZbitModel {
    num_inputs: u32,
    nodes: Vec<Node>,
    node_index: HashMap<NodeKey, u32>,
    pin_ids: Vec<u32>,
    root_id: u32,
    implicant_count: u32,
    literal_count: u32,
}

impl ZbitModel {
    pub fn new(num_inputs: u32) -> ZbitResult<Self> {
        if num_inputs > ZBIT_MAX_INPUTS_SUPPORTED {
            return Err(ZbitError::Limit(format!(
                "num_inputs={num_inputs} exceeds supported max {ZBIT_MAX_INPUTS_SUPPORTED}"
            )));
        }

        let mut model = Self {
            num_inputs,
            nodes: Vec::new(),
            node_index: HashMap::new(),
            pin_ids: Vec::new(),
            root_id: 0,
            implicant_count: 0,
            literal_count: 0,
        };
        model.reset_graph()?;
        Ok(model)
    }

    pub fn num_inputs(&self) -> u32 {
        self.num_inputs
    }

    pub fn root_id(&self) -> u32 {
        self.root_id
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    fn table_size(&self) -> ZbitResult<usize> {
        if self.num_inputs >= usize::BITS {
            return Err(ZbitError::Limit(
                "input width cannot be represented in usize truth table size".to_string(),
            ));
        }
        Ok(1usize << self.num_inputs)
    }

    fn reset_graph(&mut self) -> ZbitResult<()> {
        self.nodes.clear();
        self.node_index.clear();
        self.pin_ids.clear();
        self.pin_ids.reserve(self.num_inputs as usize);

        for i in 0..self.num_inputs {
            let id = self.intern_node_raw(NodeType::Pin, i as i32, &[])?;
            self.pin_ids.push(id);
        }

        self.root_id = self.false_id()?;
        self.implicant_count = 0;
        self.literal_count = 0;
        Ok(())
    }

    fn true_id(&mut self) -> ZbitResult<u32> {
        self.intern_node_raw(NodeType::And, -1, &[])
    }

    fn false_id(&mut self) -> ZbitResult<u32> {
        self.intern_node_raw(NodeType::Or, -1, &[])
    }

    fn intern_node_raw(
        &mut self,
        node_type: NodeType,
        value: i32,
        inputs: &[u32],
    ) -> ZbitResult<u32> {
        let key = NodeKey {
            node_type,
            value,
            inputs: inputs.to_vec(),
        };

        if let Some(id) = self.node_index.get(&key) {
            return Ok(*id);
        }

        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            node_type,
            value,
            inputs: inputs.to_vec(),
        });
        self.node_index.insert(key, id);
        Ok(id)
    }

    fn is_true_id(&self, id: u32) -> bool {
        self.nodes
            .get(id as usize)
            .map(|n| n.node_type == NodeType::And && n.inputs.is_empty())
            .unwrap_or(false)
    }

    fn is_false_id(&self, id: u32) -> bool {
        self.nodes
            .get(id as usize)
            .map(|n| n.node_type == NodeType::Or && n.inputs.is_empty())
            .unwrap_or(false)
    }

    fn intern_node(&mut self, node_type: NodeType, value: i32, inputs: &[u32]) -> ZbitResult<u32> {
        if node_type == NodeType::Pin && !inputs.is_empty() {
            return Err(ZbitError::InvalidArg("PIN cannot have inputs"));
        }
        if node_type == NodeType::Not && inputs.len() != 1 {
            return Err(ZbitError::InvalidArg("NOT must have exactly one input"));
        }

        for &id in inputs {
            if id as usize >= self.nodes.len() {
                return Err(ZbitError::Internal("input id out of range".to_string()));
            }
        }

        let mut scratch = inputs.to_vec();

        if node_type.is_commutative() && scratch.len() > 1 {
            scratch.sort_unstable();

            if node_type == NodeType::Xor {
                let mut odd = Vec::with_capacity(scratch.len());
                let mut i = 0usize;
                while i < scratch.len() {
                    let mut j = i + 1;
                    while j < scratch.len() && scratch[j] == scratch[i] {
                        j += 1;
                    }
                    if (j - i) % 2 == 1 {
                        odd.push(scratch[i]);
                    }
                    i = j;
                }
                scratch = odd;
            } else {
                scratch.dedup();
            }
        }

        if node_type == NodeType::Not {
            let arg = scratch[0];
            let arg_node = &self.nodes[arg as usize];

            if arg_node.node_type == NodeType::Not && arg_node.inputs.len() == 1 {
                return Ok(arg_node.inputs[0]);
            }
            if self.is_true_id(arg) {
                return self.false_id();
            }
            if self.is_false_id(arg) {
                return self.true_id();
            }
        }

        if matches!(node_type, NodeType::And | NodeType::Or) {
            let mut reduced = Vec::with_capacity(scratch.len());

            for arg in scratch {
                if node_type == NodeType::And {
                    if self.is_false_id(arg) {
                        return self.false_id();
                    }
                    if self.is_true_id(arg) {
                        continue;
                    }
                } else {
                    if self.is_true_id(arg) {
                        return self.true_id();
                    }
                    if self.is_false_id(arg) {
                        continue;
                    }
                }
                reduced.push(arg);
            }

            let id_set = reduced.iter().copied().collect::<HashSet<_>>();
            for &arg in &reduced {
                let node = &self.nodes[arg as usize];
                if node.node_type == NodeType::Not
                    && node.inputs.len() == 1
                    && id_set.contains(&node.inputs[0])
                {
                    return if node_type == NodeType::And {
                        self.false_id()
                    } else {
                        self.true_id()
                    };
                }
            }

            scratch = reduced;
        }

        if node_type == NodeType::Xor {
            let mut parity_true = false;
            let mut reduced = Vec::with_capacity(scratch.len());

            for arg in scratch {
                if self.is_false_id(arg) {
                    continue;
                }
                if self.is_true_id(arg) {
                    parity_true = !parity_true;
                    continue;
                }
                reduced.push(arg);
            }

            scratch = reduced;

            if scratch.is_empty() {
                return if parity_true {
                    self.true_id()
                } else {
                    self.false_id()
                };
            }

            if scratch.len() == 1 && !parity_true {
                return Ok(scratch[0]);
            }

            if parity_true {
                let true_id = self.true_id()?;
                scratch.push(true_id);
                scratch.sort_unstable();
            }
        }

        if matches!(node_type, NodeType::And | NodeType::Or | NodeType::Xor) && scratch.len() == 1 {
            return Ok(scratch[0]);
        }

        self.intern_node_raw(node_type, value, &scratch)
    }

    fn build_from_implicants(&mut self, implicants: &[Implicant]) -> ZbitResult<()> {
        self.reset_graph()?;

        if implicants.is_empty() {
            return Ok(());
        }

        let true_id = self.true_id()?;
        let mut or_inputs = Vec::with_capacity(implicants.len());

        for imp in implicants {
            if imp.mask == 0 {
                or_inputs.push(true_id);
                continue;
            }

            let mut and_inputs = Vec::new();
            for bit in 0..self.num_inputs {
                let flag = 1u32 << bit;
                if (imp.mask & flag) == 0 {
                    continue;
                }

                let pin_id = self.pin_ids[bit as usize];
                if (imp.value & flag) != 0 {
                    and_inputs.push(pin_id);
                } else {
                    let not_id = self.intern_node(NodeType::Not, -1, &[pin_id])?;
                    and_inputs.push(not_id);
                }
            }

            let and_id = self.intern_node(NodeType::And, -1, &and_inputs)?;
            or_inputs.push(and_id);
        }

        self.root_id = self.intern_node(NodeType::Or, -1, &or_inputs)?;
        Ok(())
    }

    fn collect_sets_from_table(
        &self,
        outputs: &[u8],
        dont_cares: Option<&[u8]>,
        require_exact_bound: bool,
    ) -> ZbitResult<(Vec<u32>, Vec<u32>)> {
        let table_size = self.table_size()?;
        if outputs.len() != table_size {
            return Err(ZbitError::InvalidArg(
                "outputs length must be exactly 2^num_inputs",
            ));
        }

        if require_exact_bound && self.num_inputs > ZBIT_MAX_INPUTS_EXACT {
            return Err(ZbitError::Limit(format!(
                "exact minimization supports up to {ZBIT_MAX_INPUTS_EXACT} inputs"
            )));
        }

        if let Some(dc) = dont_cares {
            if dc.len() != table_size {
                return Err(ZbitError::InvalidArg(
                    "dont_cares length must match outputs length",
                ));
            }
        }

        let mut on_set = Vec::new();
        let mut dc_set = Vec::new();

        for idx in 0..table_size {
            if dont_cares.map(|dc| dc[idx] != 0).unwrap_or(false) {
                dc_set.push(idx as u32);
            } else if outputs[idx] != 0 {
                on_set.push(idx as u32);
            }
        }

        Ok((on_set, dc_set))
    }

    pub fn compress_from_table(
        &mut self,
        outputs: &[u8],
        dont_cares: Option<&[u8]>,
    ) -> ZbitResult<()> {
        let (on_set, dc_set) = self.collect_sets_from_table(outputs, dont_cares, true)?;

        let (implicants, literal_count) = minimize_exact(self.num_inputs, &on_set, &dc_set)?;
        self.build_from_implicants(&implicants)?;
        self.implicant_count = implicants.len() as u32;
        self.literal_count = literal_count;
        Ok(())
    }

    pub fn compress_from_table_advanced(
        &mut self,
        outputs: &[u8],
        dont_cares: Option<&[u8]>,
        options: &AdvancedOptions,
    ) -> ZbitResult<AdvancedReport> {
        let (on_set, dc_set) = self.collect_sets_from_table(outputs, dont_cares, false)?;

        let result = minimize_advanced(self.num_inputs, &on_set, &dc_set, options)?;
        self.build_from_implicants(&result.implicants)?;
        self.implicant_count = result.implicants.len() as u32;
        self.literal_count = result.literal_count;
        Ok(result.report)
    }

    pub fn compress_from_table_with_objective(
        &mut self,
        outputs: &[u8],
        dont_cares: Option<&[u8]>,
        objective: MappingObjective,
    ) -> ZbitResult<AdvancedReport> {
        let options = AdvancedOptions {
            objective,
            ..AdvancedOptions::default()
        };
        self.compress_from_table_advanced(outputs, dont_cares, &options)
    }

    pub fn evaluate(&self, input_vector: u32) -> ZbitResult<bool> {
        let mut values = vec![false; self.nodes.len()];

        for (id, node) in self.nodes.iter().enumerate() {
            values[id] = match node.node_type {
                NodeType::Pin => {
                    let bit = node.value as u32;
                    bit < 32 && ((input_vector >> bit) & 1) != 0
                }
                NodeType::Not => {
                    let arg = node.inputs[0] as usize;
                    !values[arg]
                }
                NodeType::And => node.inputs.iter().all(|&arg| values[arg as usize]),
                NodeType::Or => node.inputs.iter().any(|&arg| values[arg as usize]),
                NodeType::Xor => node
                    .inputs
                    .iter()
                    .fold(false, |acc, &arg| acc ^ values[arg as usize]),
            };
        }

        values
            .get(self.root_id as usize)
            .copied()
            .ok_or_else(|| ZbitError::Internal("root id out of range".to_string()))
    }

    pub fn decompress_to_table(&self) -> ZbitResult<Vec<u8>> {
        let table_size = self.table_size()?;
        let mut table = vec![0u8; table_size];

        for (idx, out) in table.iter_mut().enumerate() {
            *out = if self.evaluate(idx as u32)? { 1 } else { 0 };
        }

        Ok(table)
    }

    pub fn validate_against_table(&self, expected: &[u8]) -> ZbitResult<()> {
        let actual = self.decompress_to_table()?;
        if expected.len() != actual.len() {
            return Err(ZbitError::InvalidArg(
                "expected table length must match model table size",
            ));
        }

        for (idx, (exp, got)) in expected.iter().zip(actual.iter()).enumerate() {
            let exp_norm = if *exp == 0 { 0 } else { 1 };
            let got_norm = if *got == 0 { 0 } else { 1 };
            if exp_norm != got_norm {
                return Err(ZbitError::ValidationMismatch {
                    index: idx,
                    expected: exp_norm,
                    actual: got_norm,
                });
            }
        }

        Ok(())
    }

    pub fn stats(&self) -> ZbitStats {
        let mut stats = ZbitStats {
            num_inputs: self.num_inputs,
            node_count: self.nodes.len() as u32,
            root_id: self.root_id,
            implicant_count: self.implicant_count,
            literal_count: self.literal_count,
            ..ZbitStats::default()
        };

        for node in &self.nodes {
            match node.node_type {
                NodeType::Pin => stats.pin_count += 1,
                NodeType::Not => stats.not_count += 1,
                NodeType::And => stats.and_count += 1,
                NodeType::Or => stats.or_count += 1,
                NodeType::Xor => stats.xor_count += 1,
            }
        }

        stats
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.nodes.len() * 16);

        push_u32(&mut out, ZBIT_MAGIC);
        push_u16(&mut out, ZBIT_VERSION);
        push_u16(&mut out, 0);
        push_u32(&mut out, self.num_inputs);
        push_u32(&mut out, self.root_id);
        push_u32(&mut out, self.nodes.len() as u32);

        for node in &self.nodes {
            out.push(node.node_type as u8);
            push_u32(&mut out, node.value as u32);
            push_u32(&mut out, node.inputs.len() as u32);
            for &arg in &node.inputs {
                push_u32(&mut out, arg);
            }
        }

        out
    }

    pub fn from_bytes(bytes: &[u8]) -> ZbitResult<Self> {
        let mut cursor = 0usize;

        let magic = read_u32(bytes, &mut cursor)?;
        if magic != ZBIT_MAGIC {
            return Err(ZbitError::Parse("invalid zbit magic".to_string()));
        }

        let version = read_u16(bytes, &mut cursor)?;
        if version != ZBIT_VERSION {
            return Err(ZbitError::Parse(format!(
                "unsupported zbit version: {version}"
            )));
        }

        let _reserved = read_u16(bytes, &mut cursor)?;
        let num_inputs = read_u32(bytes, &mut cursor)?;
        if num_inputs > ZBIT_MAX_INPUTS_SUPPORTED {
            return Err(ZbitError::Parse("num_inputs out of range".to_string()));
        }

        let root_id = read_u32(bytes, &mut cursor)?;
        let node_count = read_u32(bytes, &mut cursor)? as usize;

        let mut nodes = Vec::with_capacity(node_count);
        for idx in 0..node_count {
            let node_type = NodeType::from_u8(read_u8(bytes, &mut cursor)?)
                .ok_or_else(|| ZbitError::Parse("unknown node type".to_string()))?;

            let value = read_u32(bytes, &mut cursor)? as i32;
            let input_count = read_u32(bytes, &mut cursor)? as usize;

            if node_type == NodeType::Pin && input_count != 0 {
                return Err(ZbitError::Parse("PIN with non-zero inputs".to_string()));
            }
            if node_type == NodeType::Not && input_count != 1 {
                return Err(ZbitError::Parse("NOT with invalid arity".to_string()));
            }

            let mut inputs = Vec::with_capacity(input_count);
            for _ in 0..input_count {
                let arg = read_u32(bytes, &mut cursor)?;
                if arg as usize >= idx {
                    return Err(ZbitError::Parse(
                        "node arguments must reference earlier nodes".to_string(),
                    ));
                }
                inputs.push(arg);
            }

            if node_type.is_commutative() && inputs.len() > 1 {
                inputs.sort_unstable();
            }

            nodes.push(Node {
                node_type,
                value,
                inputs,
            });
        }

        if cursor != bytes.len() {
            return Err(ZbitError::Parse(
                "trailing bytes in serialized model".to_string(),
            ));
        }

        if root_id as usize >= nodes.len() {
            return Err(ZbitError::Parse("root id out of range".to_string()));
        }

        let mut node_index = HashMap::new();
        for (idx, node) in nodes.iter().enumerate() {
            let key = NodeKey {
                node_type: node.node_type,
                value: node.value,
                inputs: node.inputs.clone(),
            };
            if node_index.insert(key, idx as u32).is_some() {
                return Err(ZbitError::Parse(
                    "duplicate canonical node in serialized stream".to_string(),
                ));
            }
        }

        let mut pin_ids = vec![u32::MAX; num_inputs as usize];
        for (idx, node) in nodes.iter().enumerate() {
            if node.node_type != NodeType::Pin {
                continue;
            }
            if node.value < 0 {
                continue;
            }
            let pin = node.value as usize;
            if pin < pin_ids.len() && pin_ids[pin] == u32::MAX {
                pin_ids[pin] = idx as u32;
            }
        }

        if pin_ids.iter().any(|&id| id == u32::MAX) {
            return Err(ZbitError::Parse(
                "missing required pin nodes in serialized model".to_string(),
            ));
        }

        Ok(Self {
            num_inputs,
            nodes,
            node_index,
            pin_ids,
            root_id,
            implicant_count: 0,
            literal_count: 0,
        })
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> ZbitResult<()> {
        fs::write(path, self.to_bytes()).map_err(Into::into)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> ZbitResult<Self> {
        let bytes = fs::read(path)?;
        Self::from_bytes(&bytes)
    }
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u8> {
    let b = *bytes
        .get(*cursor)
        .ok_or_else(|| ZbitError::Parse("unexpected end of input".to_string()))?;
    *cursor += 1;
    Ok(b)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u16> {
    let slice = bytes
        .get(*cursor..(*cursor + 2))
        .ok_or_else(|| ZbitError::Parse("unexpected end of input".to_string()))?;
    *cursor += 2;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u32> {
    let slice = bytes
        .get(*cursor..(*cursor + 4))
        .ok_or_else(|| ZbitError::Parse("unexpected end of input".to_string()))?;
    *cursor += 4;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_roundtrip_model() {
        let mut model = ZbitModel::new(2).expect("new model");
        let outputs = [0u8, 1, 1, 0];
        model
            .compress_from_table(&outputs, None)
            .expect("compress xor");
        model
            .validate_against_table(&outputs)
            .expect("validate xor");

        let bytes = model.to_bytes();
        let loaded = ZbitModel::from_bytes(&bytes).expect("load model bytes");
        loaded
            .validate_against_table(&outputs)
            .expect("validate loaded xor");
    }
}
