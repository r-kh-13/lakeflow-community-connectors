use std::collections::HashMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::copybook::{Field, Layout, parse_field_line};
use crate::options::DecodeOptions;

#[derive(Clone, Debug)]
pub enum NodeKind {
    Primitive(Field),
    Group(Vec<Node>),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub level: u32,
    pub name: String,
    pub occurs_min: usize,
    pub occurs_max: usize,
    pub occurs_explicit: bool,
    pub depending_on: Option<String>,
    pub redefines: Option<String>,
    pub offset: usize,
    pub size: usize,
    pub kind: NodeKind,
}

impl Node {
    pub fn spark_type(&self, options: &DecodeOptions) -> String {
        let inner = match &self.kind {
            NodeKind::Primitive(field) => {
                let mut single = field.clone();
                single.occurs = 1;
                single.spark_type(options)
            }
            NodeKind::Group(children) => {
                let fields = children
                    .iter()
                    .filter(|child| !child.name.eq_ignore_ascii_case("FILLER"))
                    .map(|child| format!("{}:{}", child.name, child.spark_type(options)))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("struct<{fields}>")
            }
        };
        if self.occurs_explicit {
            format!("array<{inner}>")
        } else {
            inner
        }
    }
}

pub fn parse_layout(source: &str) -> PyResult<Layout> {
    let mut parsed = Vec::new();
    for statement in logical_statements(source) {
        if let Some(entry) = parse_entry(&statement)? {
            parsed.push(entry);
        }
    }
    if parsed.is_empty() {
        return Err(PyValueError::new_err("copybook contains no fields"));
    }
    let mut index = 0usize;
    let mut roots = build_tree(&mut parsed, &mut index, 0)?;
    let mut nodes = if roots.len() == 1
        && roots[0].level == 1
        && !roots[0].occurs_explicit
        && roots[0].redefines.is_none()
    {
        match roots.remove(0).kind {
            NodeKind::Group(children) => children,
            primitive => vec![Node {
                level: 1,
                name: "RECORD".to_string(),
                occurs_min: 1,
                occurs_max: 1,
                occurs_explicit: false,
                depending_on: None,
                redefines: None,
                offset: 0,
                size: 0,
                kind: primitive,
            }],
        }
    } else {
        roots
    };
    let record_size = assign_static_offsets(&mut nodes, 0);
    let mut fields = Vec::new();
    flatten_primitives(&nodes, &mut fields);
    Ok(Layout {
        fields,
        nodes,
        record_size,
    })
}

fn logical_statements(source: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    for raw in source.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('*') || trimmed.starts_with("*>") {
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(trimmed);
        if trimmed.ends_with('.') {
            statements.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        statements.push(current);
    }
    statements
}

fn parse_entry(statement: &str) -> PyResult<Option<Node>> {
    let tokens: Vec<&str> = statement.trim_end_matches('.').split_whitespace().collect();
    if tokens.len() < 2 || !tokens[0].chars().all(|ch| ch.is_ascii_digit()) {
        return Ok(None);
    }
    let level: u32 = tokens[0]
        .parse()
        .map_err(|_| PyValueError::new_err(format!("invalid level: {}", tokens[0])))?;
    if matches!(level, 66 | 88) {
        return Ok(None);
    }
    let name = normalize_name(tokens[1]);
    let mut occurs_min = 1usize;
    let mut occurs_max = 1usize;
    let mut occurs_explicit = false;
    let mut depending_on = None;
    let mut redefines = None;
    let mut index = 2usize;
    while index < tokens.len() {
        match tokens[index].to_ascii_uppercase().as_str() {
            "REDEFINES" => {
                index += 1;
                redefines =
                    Some(normalize_name(tokens.get(index).ok_or_else(|| {
                        PyValueError::new_err("REDEFINES missing target")
                    })?));
            }
            "OCCURS" => {
                occurs_explicit = true;
                index += 1;
                occurs_min = tokens
                    .get(index)
                    .ok_or_else(|| PyValueError::new_err("OCCURS missing count"))?
                    .parse()
                    .map_err(|_| PyValueError::new_err("invalid OCCURS count"))?;
                occurs_max = occurs_min;
                if tokens
                    .get(index + 1)
                    .is_some_and(|token| token.eq_ignore_ascii_case("TO"))
                {
                    index += 2;
                    occurs_max = tokens
                        .get(index)
                        .ok_or_else(|| PyValueError::new_err("OCCURS TO missing maximum"))?
                        .parse()
                        .map_err(|_| PyValueError::new_err("invalid OCCURS maximum"))?;
                }
                if occurs_min > occurs_max {
                    return Err(PyValueError::new_err(
                        "OCCURS minimum cannot exceed maximum",
                    ));
                }
            }
            "DEPENDING" => {
                if !tokens
                    .get(index + 1)
                    .is_some_and(|token| token.eq_ignore_ascii_case("ON"))
                {
                    return Err(PyValueError::new_err("DEPENDING must be followed by ON"));
                }
                index += 2;
                depending_on =
                    Some(normalize_name(tokens.get(index).ok_or_else(|| {
                        PyValueError::new_err("DEPENDING ON missing field")
                    })?));
            }
            _ => {}
        }
        index += 1;
    }
    let primitive = parse_field_line(statement)?;
    let kind = if let Some(mut field) = primitive {
        field.occurs = occurs_max;
        field.redefines.clone_from(&redefines);
        NodeKind::Primitive(field)
    } else {
        NodeKind::Group(Vec::new())
    };
    Ok(Some(Node {
        level,
        name,
        occurs_min,
        occurs_max,
        occurs_explicit,
        depending_on,
        redefines,
        offset: 0,
        size: 0,
        kind,
    }))
}

fn build_tree(entries: &mut [Node], index: &mut usize, parent_level: u32) -> PyResult<Vec<Node>> {
    let mut result = Vec::new();
    while *index < entries.len() {
        let level = entries[*index].level;
        if level <= parent_level {
            break;
        }
        let mut node = entries[*index].clone();
        *index += 1;
        if *index < entries.len() && entries[*index].level > level {
            let children = build_tree(entries, index, level)?;
            match &mut node.kind {
                NodeKind::Group(existing) => *existing = children,
                NodeKind::Primitive(_) => {
                    return Err(PyValueError::new_err(format!(
                        "primitive field {} cannot contain subordinate fields",
                        node.name
                    )));
                }
            }
        }
        result.push(node);
    }
    Ok(result)
}

fn assign_static_offsets(nodes: &mut [Node], start: usize) -> usize {
    let mut cursor = start;
    let mut starts: HashMap<String, usize> = HashMap::new();
    for node in nodes {
        let node_start = node
            .redefines
            .as_ref()
            .and_then(|target| starts.get(target))
            .copied()
            .unwrap_or(cursor);
        node.offset = node_start;
        node.size = match &mut node.kind {
            NodeKind::Primitive(field) => {
                field.offset = node_start;
                field.occurs = node.occurs_max;
                field.size
            }
            NodeKind::Group(children) => assign_static_offsets(children, node_start) - node_start,
        };
        starts.insert(node.name.clone(), node_start);
        cursor = cursor.max(node_start + node.size * node.occurs_max);
    }
    cursor
}

fn flatten_primitives(nodes: &[Node], output: &mut Vec<Field>) {
    for node in nodes {
        match &node.kind {
            NodeKind::Primitive(field) => output.push(field.clone()),
            NodeKind::Group(children) => flatten_primitives(children, output),
        }
    }
}

fn normalize_name(value: &str) -> String {
    value
        .trim_matches(|ch: char| ch == '.' || ch == '\'' || ch == '"')
        .replace('-', "_")
        .to_ascii_uppercase()
}
