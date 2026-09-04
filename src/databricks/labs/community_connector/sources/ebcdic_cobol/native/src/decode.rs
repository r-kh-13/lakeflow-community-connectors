// COBOL numeric decoding behavior adapted from AbsaOSS/cobrix.
// Copyright 2018 ABSA Group Limited. Apache License 2.0.
// See ../../THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;

use crate::copybook::{Field, Layout, Pic, SignSeparate, Usage};
use crate::ebcdic;
use crate::float;
use crate::options::{DecodeOptions, StringTrim, TextEncoding};
use crate::structure::{Node, NodeKind};
use pyo3::IntoPyObjectExt;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple, PyType};

static DECIMAL_TYPE: PyOnceLock<Py<PyType>> = PyOnceLock::new();

pub fn decode_layout<'py>(
    py: Python<'py>,
    layout: &Layout,
    record: &[u8],
    options: &DecodeOptions,
) -> PyResult<Bound<'py, PyDict>> {
    Ok(decode_layout_with_consumed(py, layout, record, options)?.0)
}

pub fn decode_layout_with_consumed<'py>(
    py: Python<'py>,
    layout: &Layout,
    record: &[u8],
    options: &DecodeOptions,
) -> PyResult<(Bound<'py, PyDict>, usize)> {
    let mut state = DecodeState::default();
    let values = PyDict::new(py);
    decode_scope(py, &layout.nodes, record, 0, options, &mut state, values)
}

pub fn decode_layout_tuple_with_consumed<'py>(
    py: Python<'py>,
    layout: &Layout,
    record: &[u8],
    options: &DecodeOptions,
) -> PyResult<(Bound<'py, PyTuple>, usize)> {
    let mut state = DecodeState::default();
    let mut cursor = 0usize;
    let mut sibling_starts: HashMap<&str, usize> = HashMap::new();
    let mut values = Vec::with_capacity(layout.nodes.len());
    for node in &layout.nodes {
        let node_start = node
            .redefines
            .as_deref()
            .and_then(|target| sibling_starts.get(target))
            .copied()
            .unwrap_or(cursor);
        let (decoded, consumed) = decode_node(py, node, record, node_start, options, &mut state)?;
        if !node.name.eq_ignore_ascii_case("FILLER") {
            values.push(decoded);
        }
        sibling_starts.insert(&node.name, node_start);
        cursor = cursor.max(node_start + consumed);
    }
    Ok((PyTuple::new(py, values)?, cursor))
}

#[derive(Default)]
struct DecodeState {
    numeric_values: HashMap<String, i64>,
}

fn decode_scope<'py>(
    py: Python<'py>,
    nodes: &[Node],
    record: &[u8],
    start: usize,
    options: &DecodeOptions,
    state: &mut DecodeState,
    values: Bound<'py, PyDict>,
) -> PyResult<(Bound<'py, PyDict>, usize)> {
    let mut cursor = start;
    let mut sibling_starts: HashMap<&str, usize> = HashMap::new();
    for node in nodes {
        let node_start = node
            .redefines
            .as_deref()
            .and_then(|target| sibling_starts.get(target))
            .copied()
            .unwrap_or(cursor);
        let (decoded, consumed) = decode_node(py, node, record, node_start, options, state)?;
        if !node.name.eq_ignore_ascii_case("FILLER") {
            values.set_item(&node.name, decoded)?;
        }
        sibling_starts.insert(&node.name, node_start);
        cursor = cursor.max(node_start + consumed);
    }
    Ok((values, cursor))
}

fn decode_node<'py>(
    py: Python<'py>,
    node: &Node,
    record: &[u8],
    start: usize,
    options: &DecodeOptions,
    state: &mut DecodeState,
) -> PyResult<(Bound<'py, PyAny>, usize)> {
    let count = occurrence_count(node, state)?;
    match &node.kind {
        NodeKind::Primitive(field) if node.occurs_explicit => {
            let items = PyList::empty(py);
            for index in 0..count {
                let value = decode_one(py, field, record, start + index * field.size, options)?;
                remember_numeric(node, &value, state);
                items.append(value)?;
            }
            Ok((items.into_any(), field.size * count))
        }
        NodeKind::Primitive(field) => {
            let value = decode_one(py, field, record, start, options)?;
            remember_numeric(node, &value, state);
            Ok((value, field.size))
        }
        NodeKind::Group(children) if node.occurs_explicit => {
            let items = PyList::empty(py);
            let mut cursor = start;
            for _ in 0..count {
                let child_values = PyDict::new(py);
                let (decoded, end) =
                    decode_scope(py, children, record, cursor, options, state, child_values)?;
                items.append(decoded)?;
                cursor = end;
            }
            Ok((items.into_any(), cursor - start))
        }
        NodeKind::Group(children) => {
            let child_values = PyDict::new(py);
            let (decoded, end) =
                decode_scope(py, children, record, start, options, state, child_values)?;
            Ok((decoded.into_any(), end - start))
        }
    }
}

fn occurrence_count(node: &Node, state: &DecodeState) -> PyResult<usize> {
    let Some(depending_on) = node.depending_on.as_ref() else {
        return Ok(node.occurs_max);
    };
    let value = state.numeric_values.get(depending_on).ok_or_else(|| {
        PyValueError::new_err(format!(
            "OCCURS DEPENDING ON field {depending_on} has not been decoded"
        ))
    })?;
    let count = usize::try_from(*value).map_err(|_| {
        PyValueError::new_err(format!(
            "OCCURS DEPENDING ON {depending_on} cannot be negative"
        ))
    })?;
    if !(node.occurs_min..=node.occurs_max).contains(&count) {
        return Err(PyValueError::new_err(format!(
            "OCCURS DEPENDING ON {depending_on}={count} is outside {}..={}",
            node.occurs_min, node.occurs_max
        )));
    }
    Ok(count)
}

fn remember_numeric(node: &Node, value: &Bound<'_, PyAny>, state: &mut DecodeState) {
    if let Ok(number) = value.extract::<i64>() {
        state.numeric_values.insert(node.name.clone(), number);
    }
}

fn decode_one<'py>(
    py: Python<'py>,
    field: &Field,
    record: &[u8],
    offset: usize,
    options: &DecodeOptions,
) -> PyResult<Bound<'py, PyAny>> {
    let end = offset + field.size;
    if end > record.len() {
        if options.null_on_error {
            return Ok(py.None().into_bound(py));
        }
        return Err(PyValueError::new_err(format!(
            "field {} truncated at offset {offset}",
            field.name
        )));
    }
    let bytes = &record[offset..end];
    if options.improved_null_detection {
        let is_null = match field.pic {
            Pic::Alpha { .. } => match options.text_encoding {
                TextEncoding::Ebcdic | TextEncoding::Ascii => {
                    options.string_trim != StringTrim::KeepAll
                        && bytes.iter().all(|value| *value == 0)
                }
                TextEncoding::Utf16 => bytes.iter().all(|value| *value == 0),
                TextEncoding::Hex | TextEncoding::Raw => false,
            },
            Pic::Numeric { .. } if field.usage == Usage::Display => {
                bytes.iter().all(|value| matches!(*value, 0 | 0x20 | 0x40))
            }
            _ => false,
        };
        if is_null {
            return Ok(py.None().into_bound(py));
        }
    }

    let decoded = match (&field.pic, &field.usage) {
        (_, Usage::Comp1) | (Pic::Float { bits: 32 }, _) => {
            float::decode_single(bytes, options.floating_point_format)
                .and_then(|value| value.into_bound_py_any(py))
        }
        (_, Usage::Comp2) | (Pic::Float { bits: 64 }, _) => {
            float::decode_double(bytes, options.floating_point_format)
                .and_then(|value| value.into_bound_py_any(py))
        }
        (Pic::Alpha { .. }, Usage::Comp) => Ok(PyBytes::new(py, bytes).into_any()),
        (Pic::Alpha { .. }, _) => decode_alpha(py, bytes, options),
        (
            Pic::Numeric {
                signed,
                scale,
                scale_factor,
                sign_separate,
                ..
            },
            Usage::Display,
        ) => decode_display_number(
            bytes,
            *signed,
            *scale,
            *scale_factor,
            sign_separate,
            options,
        )
        .and_then(|value| {
            if options.display_pic_as_string && *scale == 0 && *scale_factor == 0 {
                value.into_bound_py_any(py)
            } else {
                number_to_py(
                    py,
                    &value,
                    field.precision(),
                    *scale,
                    *scale_factor,
                    options.strict_integral_precision,
                )
            }
        }),
        (
            Pic::Numeric {
                scale,
                scale_factor,
                ..
            },
            Usage::Comp3 | Usage::Comp3U,
        ) => decode_bcd(
            bytes,
            *scale,
            *scale_factor,
            matches!(field.usage, Usage::Comp3),
        )
        .and_then(|value| {
            number_to_py(
                py,
                &value,
                field.precision(),
                *scale,
                *scale_factor,
                options.strict_integral_precision,
            )
        }),
        (
            Pic::Numeric {
                signed,
                int_digits,
                scale,
                scale_factor,
                ..
            },
            Usage::Comp | Usage::Comp9,
        ) => decode_comp_binary(
            bytes,
            *signed,
            matches!(field.usage, Usage::Comp),
            int_digits + scale,
        )
        .and_then(|value| {
            let scaled = add_decimal_point(&value, *scale, *scale_factor);
            number_to_py(
                py,
                &scaled,
                field.precision(),
                *scale,
                *scale_factor,
                options.strict_integral_precision,
            )
        }),
        _ => Err(PyValueError::new_err(format!(
            "unsupported mapping for field {}",
            field.name
        ))),
    };
    match decoded {
        Ok(value) => Ok(value),
        Err(_) if options.null_on_error => Ok(py.None().into_bound(py)),
        Err(error) => Err(error),
    }
}

fn decode_alpha<'py>(
    py: Python<'py>,
    bytes: &[u8],
    options: &DecodeOptions,
) -> PyResult<Bound<'py, PyAny>> {
    match options.text_encoding {
        TextEncoding::Raw => Ok(PyBytes::new(py, bytes).into_any()),
        TextEncoding::Hex => bytes
            .iter()
            .map(|value| format!("{value:02X}"))
            .collect::<String>()
            .into_bound_py_any(py),
        TextEncoding::Ebcdic => options
            .string_trim
            .apply(ebcdic::decode_string(bytes))
            .into_bound_py_any(py),
        TextEncoding::Ascii => options
            .string_trim
            .apply(decode_ascii_string(bytes, options.string_trim))
            .into_bound_py_any(py),
        TextEncoding::Utf16 => options
            .string_trim
            .apply(decode_utf16(bytes, options.utf16_big_endian)?)
            .into_bound_py_any(py),
    }
}

fn decode_ascii_string(bytes: &[u8], trimming: StringTrim) -> String {
    bytes
        .iter()
        .filter_map(|value| {
            if trimming == StringTrim::KeepAll || (32..128).contains(value) {
                Some(char::from(*value))
            } else if *value >= 128 {
                Some(' ')
            } else {
                None
            }
        })
        .collect()
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> PyResult<String> {
    if !bytes.len().is_multiple_of(2) {
        return Err(PyValueError::new_err(
            "UTF16 field byte length must be even",
        ));
    }
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if big_endian {
                u16::from_be_bytes(*pair)
            } else {
                u16::from_le_bytes(*pair)
            }
        })
        .collect::<Vec<_>>();
    Ok(String::from_utf16_lossy(&units))
}

fn number_to_py<'py>(
    py: Python<'py>,
    rendered: &str,
    precision: usize,
    scale: usize,
    scale_factor: i32,
    strict_integral_precision: bool,
) -> PyResult<Bound<'py, PyAny>> {
    if !strict_integral_precision
        && scale == 0
        && scale_factor == 0
        && precision <= 18
        && !rendered.contains('.')
        && let Ok(value) = rendered.parse::<i64>()
    {
        return value.into_bound_py_any(py);
    }
    DECIMAL_TYPE
        .import(py, "decimal", "Decimal")?
        .call1((rendered,))
}

fn decode_display_number(
    data: &[u8],
    signed: bool,
    scale: usize,
    scale_factor: i32,
    sign_separate: &SignSeparate,
    options: &DecodeOptions,
) -> PyResult<String> {
    if data.is_empty() {
        return Err(PyValueError::new_err("empty display numeric"));
    }
    if !matches!(
        options.text_encoding,
        TextEncoding::Ebcdic | TextEncoding::Ascii
    ) {
        return Err(PyValueError::new_err(
            "DISPLAY numeric fields require EBCDIC or ASCII encoding",
        ));
    }
    let (sign_byte, number_bytes) = match sign_separate {
        SignSeparate::Leading => (Some(data[0]), &data[1..]),
        SignSeparate::Trailing => (Some(data[data.len() - 1]), &data[..data.len() - 1]),
        SignSeparate::None => (None, data),
    };
    let mut separate_sign = None;
    if let Some(value) = sign_byte {
        let (is_plus, is_minus) = match options.text_encoding {
            TextEncoding::Ebcdic => (value == 0x4E, value == 0x60),
            TextEncoding::Ascii => (value == b'+', value == b'-'),
            _ => unreachable!(),
        };
        if is_minus {
            separate_sign = Some(-1);
        } else if is_plus {
            separate_sign = Some(1);
        } else {
            return Err(PyValueError::new_err(format!(
                "invalid separate sign byte: 0x{value:02X}"
            )));
        }
    }
    let allow_overpunch = signed || !options.strict_sign_overpunch;
    let relaxed = !options.strict_sign_overpunch;
    let (digits, embedded_sign) = match options.text_encoding {
        TextEncoding::Ebcdic => decode_ebcdic_number(number_bytes, allow_overpunch, relaxed)?,
        TextEncoding::Ascii => decode_ascii_number(number_bytes, allow_overpunch, relaxed)?,
        _ => unreachable!(),
    };
    let sign = separate_sign.or(embedded_sign).unwrap_or(1);
    if !signed && sign < 0 {
        return Err(PyValueError::new_err(
            "negative value for unsigned display numeric",
        ));
    }
    if digits.is_empty() {
        return Err(PyValueError::new_err("display numeric has no digits"));
    }
    let signed_digits = match sign {
        -1 => format!("-{digits}"),
        1 if separate_sign.is_some() || embedded_sign.is_some() => format!("+{digits}"),
        _ => digits,
    };
    Ok(add_decimal_point(&signed_digits, scale, scale_factor))
}

fn decode_ebcdic_number(
    data: &[u8],
    allow_overpunch: bool,
    relaxed: bool,
) -> PyResult<(String, Option<i8>)> {
    let mut sign = None;
    let mut digits = String::new();
    for value in data {
        let sign_seen = sign.is_some() && !relaxed;
        match *value {
            0xF0..=0xF9 => digits.push(char::from(b'0' + (value - 0xF0))),
            0xC0..=0xC9 if allow_overpunch && !sign_seen => {
                digits.push(char::from(b'0' + (value - 0xC0)));
                sign = Some(1);
            }
            0xD0..=0xD9 if allow_overpunch && !sign_seen => {
                digits.push(char::from(b'0' + (value - 0xD0)));
                sign = Some(-1);
            }
            0x60 => sign = Some(-1),
            0x4E => sign = Some(1),
            0x4B | 0x6B => digits.push('.'),
            0x40 | 0x00 => {}
            _ => {
                return Err(PyValueError::new_err(format!(
                    "invalid zoned-decimal byte: 0x{value:02X}"
                )));
            }
        }
    }
    Ok((digits, sign))
}

fn decode_ascii_number(
    data: &[u8],
    allow_overpunch: bool,
    relaxed: bool,
) -> PyResult<(String, Option<i8>)> {
    const OVERPUNCH: &[u8] = b"{ABCDEFGHI}JKLMNOPQR";
    let mut sign = None;
    let mut digits = String::new();
    for (index, value) in data.iter().enumerate() {
        match *value {
            b'0'..=b'9' => digits.push(char::from(*value)),
            b' ' | 0 => {}
            b'+' => sign = Some(1),
            b'-' => sign = Some(-1),
            b'.' | b',' => digits.push('.'),
            other => {
                let punched = OVERPUNCH.iter().position(|item| *item == other);
                let at_edge = index == 0 || index + 1 == data.len();
                if let Some(punched) = punched
                    && (relaxed || (allow_overpunch && at_edge))
                {
                    if punched >= 10 {
                        sign = Some(-1);
                        digits.push(char::from(b'0' + (punched - 10) as u8));
                    } else {
                        sign = Some(1);
                        digits.push(char::from(b'0' + punched as u8));
                    }
                } else {
                    return Err(PyValueError::new_err(format!(
                        "invalid ASCII numeric byte: 0x{other:02X}"
                    )));
                }
            }
        }
    }
    Ok((digits, sign))
}

fn decode_bcd(
    data: &[u8],
    scale: usize,
    scale_factor: i32,
    mandatory_sign: bool,
) -> PyResult<String> {
    if data.is_empty() {
        return Err(PyValueError::new_err("COMP-3 value cannot be empty"));
    }
    let mut digits = String::new();
    let mut negative = false;
    for (index, value) in data.iter().enumerate() {
        let high = value >> 4;
        let low = value & 0x0F;
        if high > 9 {
            return Err(PyValueError::new_err("invalid COMP-3 digit"));
        }
        digits.push(char::from(b'0' + high));
        if index + 1 == data.len() && mandatory_sign {
            match low {
                0x0C | 0x0F => {}
                0x0D => negative = true,
                _ => return Err(PyValueError::new_err("invalid COMP-3 sign or digit")),
            }
        } else if low > 9 {
            return Err(PyValueError::new_err("invalid COMP-3 digit"));
        } else {
            digits.push(char::from(b'0' + low));
        }
    }
    let signed = if negative {
        format!("-{digits}")
    } else {
        digits
    };
    Ok(add_decimal_point(&signed, scale, scale_factor))
}

fn decode_comp_binary(
    data: &[u8],
    signed: bool,
    big_endian: bool,
    digits: usize,
) -> PyResult<String> {
    if data.is_empty() || data.len() > 16 {
        return Err(PyValueError::new_err(format!(
            "unsupported COMP width: {}",
            data.len()
        )));
    }
    let ordered = if big_endian {
        data.to_vec()
    } else {
        data.iter().rev().copied().collect()
    };
    let rendered = if signed {
        let fill = if ordered[0] & 0x80 == 0 { 0x00 } else { 0xFF };
        let mut buffer = [fill; 16];
        buffer[16 - ordered.len()..].copy_from_slice(&ordered);
        i128::from_be_bytes(buffer).to_string()
    } else {
        let mut buffer = [0u8; 16];
        buffer[16 - ordered.len()..].copy_from_slice(&ordered);
        u128::from_be_bytes(buffer).to_string()
    };
    if rendered.trim_start_matches('-').len() > digits {
        return Err(PyValueError::new_err("COMP value exceeds PIC digits"));
    }
    Ok(rendered)
}

fn add_decimal_point(int_value: &str, scale: usize, scale_factor: i32) -> String {
    if int_value.contains('.') {
        return int_value.to_string();
    }
    if scale_factor < 0 {
        let negative = int_value.starts_with('-');
        let digits = int_value.trim_start_matches(['-', '+']);
        let zeros = "0".repeat((-scale_factor) as usize);
        return if negative {
            format!("-0.{zeros}{digits}")
        } else {
            format!("0.{zeros}{digits}")
        };
    }
    let value = if scale_factor > 0 {
        format!("{int_value}{}", "0".repeat(scale_factor as usize))
    } else {
        int_value.to_string()
    };
    if scale == 0 {
        return value;
    }
    let negative = value.starts_with('-');
    let positive = value.starts_with('+');
    let digits = value.trim_start_matches(['-', '+']);
    let (left, right) = if digits.len() > scale {
        digits.split_at(digits.len() - scale)
    } else {
        return format!(
            "{}0.{}{}",
            if negative {
                "-"
            } else if positive {
                "+"
            } else {
                ""
            },
            "0".repeat(scale - digits.len()),
            digits
        );
    };
    format!(
        "{}{left}.{right}",
        if negative {
            "-"
        } else if positive {
            "+"
        } else {
            ""
        }
    )
}
