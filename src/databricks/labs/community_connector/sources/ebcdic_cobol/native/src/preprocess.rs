use std::collections::HashMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

pub fn expand_copybook(source: &str, library: &HashMap<String, String>) -> PyResult<String> {
    expand_recursive(source, library, &mut Vec::new())
}

fn expand_recursive(
    source: &str,
    library: &HashMap<String, String>,
    stack: &mut Vec<String>,
) -> PyResult<String> {
    let mut output = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index];
        if !line.trim_start().to_ascii_uppercase().starts_with("COPY ") {
            output.push(line.to_string());
            index += 1;
            continue;
        }
        let mut statement = line.trim().to_string();
        while !statement.trim_end().ends_with('.') {
            index += 1;
            let continuation = lines
                .get(index)
                .ok_or_else(|| PyValueError::new_err("unterminated COPY statement"))?;
            statement.push(' ');
            statement.push_str(continuation.trim());
        }
        index += 1;
        let (member, replacements) = parse_copy_statement(&statement)?;
        let source_key = find_member(library, &member).ok_or_else(|| {
            PyValueError::new_err(format!("COPY member {member} was not provided"))
        })?;
        let normalized = normalize_member(source_key);
        if stack.contains(&normalized) {
            let mut cycle = stack.clone();
            cycle.push(normalized);
            return Err(PyValueError::new_err(format!(
                "recursive COPY cycle: {}",
                cycle.join(" -> ")
            )));
        }
        stack.push(normalized);
        let mut expanded = expand_recursive(&library[source_key], library, stack)?;
        stack.pop();
        for (from, to) in replacements {
            expanded = expanded.replace(&from, &to);
        }
        output.push(expanded);
    }
    Ok(output.join("\n"))
}

fn parse_copy_statement(statement: &str) -> PyResult<(String, Vec<(String, String)>)> {
    let body = statement.trim().trim_end_matches('.');
    let after_copy = body
        .get(4..)
        .ok_or_else(|| PyValueError::new_err("invalid COPY statement"))?
        .trim();
    let member_end = after_copy
        .find(char::is_whitespace)
        .unwrap_or(after_copy.len());
    let member = after_copy[..member_end]
        .trim_matches(['\'', '"'])
        .to_string();
    if member.is_empty() {
        return Err(PyValueError::new_err("COPY member is empty"));
    }
    let remainder = after_copy[member_end..].trim();
    if remainder.is_empty() {
        return Ok((member, Vec::new()));
    }
    let replacing = strip_keyword(remainder, "REPLACING")
        .ok_or_else(|| PyValueError::new_err("COPY only supports REPLACING"))?;
    Ok((member, parse_replacements(replacing)?))
}

fn parse_replacements(mut input: &str) -> PyResult<Vec<(String, String)>> {
    let mut replacements = Vec::new();
    while !input.trim().is_empty() {
        let (from, rest) = parse_operand(input)?;
        let after_by = strip_keyword(rest.trim_start(), "BY")
            .ok_or_else(|| PyValueError::new_err("COPY REPLACING requires BY"))?;
        let (to, rest) = parse_operand(after_by)?;
        if from.is_empty() {
            return Err(PyValueError::new_err(
                "COPY REPLACING source cannot be empty",
            ));
        }
        replacements.push((from, to));
        input = rest;
    }
    Ok(replacements)
}

fn parse_operand(input: &str) -> PyResult<(String, &str)> {
    let input = input.trim_start();
    if let Some(rest) = input.strip_prefix("==") {
        let end = rest
            .find("==")
            .ok_or_else(|| PyValueError::new_err("unterminated COPY pseudo-text"))?;
        return Ok((rest[..end].to_string(), &rest[end + 2..]));
    }
    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    if end == 0 {
        return Err(PyValueError::new_err("missing COPY operand"));
    }
    Ok((input[..end].to_string(), &input[end..]))
}

fn strip_keyword<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    let end = keyword.len();
    if input.len() >= end
        && input[..end].eq_ignore_ascii_case(keyword)
        && input[end..].chars().next().is_none_or(char::is_whitespace)
    {
        Some(input[end..].trim_start())
    } else {
        None
    }
}

fn find_member<'a>(library: &'a HashMap<String, String>, requested: &str) -> Option<&'a String> {
    let requested = normalize_member(requested);
    library
        .keys()
        .find(|candidate| normalize_member(candidate) == requested)
}

fn normalize_member(value: &str) -> String {
    let upper = value.trim().trim_matches(['\'', '"']).to_ascii_uppercase();
    upper
        .strip_suffix(".CPY")
        .or_else(|| upper.strip_suffix(".COB"))
        .unwrap_or(&upper)
        .to_string()
}
