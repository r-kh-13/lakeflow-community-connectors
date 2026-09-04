// Copybook type and storage-size behavior adapted from AbsaOSS/cobrix.
// Copyright 2018 ABSA Group Limited. Apache License 2.0.
// See ../../THIRD_PARTY_NOTICES.md.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::options::{DecodeOptions, TextEncoding};
use crate::structure::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Usage {
    Display,
    Comp,
    Comp3,
    Comp3U,
    Comp1,
    Comp2,
    Comp9,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignSeparate {
    None,
    Leading,
    Trailing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pic {
    Alpha {
        len: usize,
    },
    Numeric {
        signed: bool,
        int_digits: usize,
        scale: usize,
        scale_factor: i32,
        explicit_decimal: bool,
        sign_separate: SignSeparate,
    },
    Float {
        bits: u8,
    },
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub pic: Pic,
    pub usage: Usage,
    pub occurs: usize,
    pub redefines: Option<String>,
    pub offset: usize,
    pub size: usize,
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub fields: Vec<Field>,
    pub nodes: Vec<Node>,
    pub record_size: usize,
}

impl Field {
    pub fn precision(&self) -> usize {
        match self.pic {
            Pic::Numeric {
                int_digits, scale, ..
            } => int_digits + scale,
            Pic::Alpha { len } => len,
            Pic::Float { bits: 32 } => 4,
            Pic::Float { bits: 64 } => 8,
            Pic::Float { .. } => 0,
        }
    }

    pub fn spark_type(&self, options: &DecodeOptions) -> String {
        let inner = match (&self.usage, &self.pic) {
            (Usage::Comp1, _) | (_, Pic::Float { bits: 32 }) => "float".to_string(),
            (Usage::Comp2, _) | (_, Pic::Float { bits: 64 }) => "double".to_string(),
            (Usage::Comp, Pic::Alpha { .. }) => "binary".to_string(),
            (_, Pic::Alpha { .. }) if options.text_encoding == TextEncoding::Raw => {
                "binary".to_string()
            }
            (_, Pic::Alpha { .. }) => "string".to_string(),
            (
                Usage::Display,
                Pic::Numeric {
                    scale,
                    scale_factor,
                    ..
                },
            ) if options.display_pic_as_string && *scale == 0 && *scale_factor == 0 => {
                "string".to_string()
            }
            (
                _,
                Pic::Numeric {
                    int_digits,
                    scale,
                    scale_factor,
                    ..
                },
            ) => numeric_spark_type(
                *int_digits,
                *scale,
                *scale_factor,
                options.strict_integral_precision,
            ),
            _ => "string".to_string(),
        };
        if self.occurs > 1 {
            format!("array<{inner}>")
        } else {
            inner
        }
    }
}

fn numeric_spark_type(
    int_digits: usize,
    scale: usize,
    scale_factor: i32,
    strict_integral_precision: bool,
) -> String {
    let precision = int_digits + scale;
    let effective_precision = precision + scale_factor.unsigned_abs() as usize;
    if scale_factor < 0 {
        return format!("decimal({effective_precision},{effective_precision})");
    }
    if scale_factor > 0 || scale == 0 {
        if strict_integral_precision {
            format!("decimal({effective_precision},0)")
        } else if effective_precision <= 9 {
            "integer".to_string()
        } else if effective_precision <= 18 {
            "long".to_string()
        } else {
            format!("decimal({effective_precision},0)")
        }
    } else {
        format!("decimal({precision},{scale})")
    }
}

pub fn parse_copybook(source: &str) -> PyResult<Layout> {
    crate::structure::parse_layout(source)
}

pub(crate) fn parse_field_line(line: &str) -> PyResult<Option<Field>> {
    let tokens: Vec<&str> = line.trim_end_matches('.').split_whitespace().collect();
    if tokens.len() < 2 || !tokens[0].chars().all(|c| c.is_ascii_digit()) {
        return Ok(None);
    }
    let level: u32 = tokens[0]
        .parse()
        .map_err(|_| PyValueError::new_err(format!("invalid copybook level: {}", tokens[0])))?;
    if level == 1 {
        return Ok(None);
    }

    let name = tokens[1].replace('-', "_");
    let mut redefines = None;
    let mut pic_token = None;
    let mut usage = Usage::Display;
    let mut occurs = 1;
    let mut sign_separate = SignSeparate::None;
    let mut index = 2;
    while index < tokens.len() {
        match tokens[index].to_ascii_uppercase().as_str() {
            "REDEFINES" => {
                index += 1;
                if index >= tokens.len() {
                    return Err(PyValueError::new_err("REDEFINES missing target"));
                }
                redefines = Some(tokens[index].replace('-', "_"));
            }
            "PIC" | "PICTURE" => {
                index += 1;
                if index >= tokens.len() {
                    return Err(PyValueError::new_err("PIC missing clause"));
                }
                pic_token = Some(tokens[index].to_ascii_uppercase());
            }
            "COMP-1" | "COMPUTATIONAL-1" => usage = Usage::Comp1,
            "COMP-2" | "COMPUTATIONAL-2" => usage = Usage::Comp2,
            "COMP-3" | "COMPUTATIONAL-3" | "PACKED-DECIMAL" => usage = Usage::Comp3,
            "COMP-3U" => usage = Usage::Comp3U,
            "COMP-9" => usage = Usage::Comp9,
            "COMP" | "COMPUTATIONAL" | "COMP-4" | "COMP-5" | "BINARY" => usage = Usage::Comp,
            "LEADING" => sign_separate = SignSeparate::Leading,
            "TRAILING" => sign_separate = SignSeparate::Trailing,
            "OCCURS" => {
                index += 1;
                if index >= tokens.len() {
                    return Err(PyValueError::new_err("OCCURS missing count"));
                }
                let first: usize = tokens[index]
                    .parse()
                    .map_err(|_| PyValueError::new_err("invalid OCCURS count"))?;
                if index + 2 < tokens.len() && tokens[index + 1].eq_ignore_ascii_case("TO") {
                    index += 2;
                    occurs = tokens[index]
                        .parse()
                        .map_err(|_| PyValueError::new_err("invalid OCCURS TO count"))?;
                } else {
                    occurs = first;
                }
            }
            "TIMES" | "DEPENDING" | "ON" | "USAGE" | "IS" | "SIGN" | "SEPARATE" | "CHARACTER" => {}
            _ => {}
        }
        index += 1;
    }

    let pic = match (pic_token, &usage) {
        (Some(pic_text), _) => {
            let mut parsed = parse_pic(&pic_text)?;
            if let Pic::Numeric {
                sign_separate: slot,
                ..
            } = &mut parsed
            {
                *slot = sign_separate.clone();
            }
            parsed
        }
        (None, Usage::Comp1) => Pic::Float { bits: 32 },
        (None, Usage::Comp2) => Pic::Float { bits: 64 },
        (None, _) => return Ok(None),
    };
    let size = storage_size(&pic, &usage);
    Ok(Some(Field {
        name,
        pic,
        usage,
        occurs,
        redefines,
        offset: 0,
        size,
    }))
}

fn parse_pic(text: &str) -> PyResult<Pic> {
    let normalized = text.trim_end_matches('.').to_ascii_uppercase();
    if normalized.starts_with('X') || normalized.starts_with('A') {
        let symbol = normalized.as_bytes()[0] as char;
        let len = pic_symbol_count(&normalized, symbol)
            .ok_or_else(|| PyValueError::new_err(format!("unsupported PIC: {text}")))?;
        return Ok(Pic::Alpha { len });
    }

    let signed = normalized.starts_with('S');
    let body = normalized.trim_start_matches('S');
    let mut int_digits = 0usize;
    let mut scale = 0usize;
    let mut scale_factor = 0i32;
    let mut seen_v = false;
    let mut explicit_decimal = false;
    let bytes = body.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'V' => {
                seen_v = true;
                index += 1;
            }
            b'.' | b',' => {
                seen_v = true;
                explicit_decimal = true;
                index += 1;
            }
            b'P' => {
                let (count, next) = read_symbol_count(bytes, index, b'P')?;
                if seen_v || int_digits == 0 {
                    scale_factor -= count as i32;
                } else {
                    scale_factor += count as i32;
                }
                index = next;
            }
            b'9' => {
                let (count, next) = read_symbol_count(bytes, index, b'9')?;
                if seen_v {
                    scale += count;
                } else {
                    int_digits += count;
                }
                index = next;
            }
            other => {
                return Err(PyValueError::new_err(format!(
                    "unsupported PIC character: {}",
                    other as char
                )));
            }
        }
    }
    if int_digits + scale == 0 {
        return Err(PyValueError::new_err(format!("unsupported PIC: {text}")));
    }
    Ok(Pic::Numeric {
        signed,
        int_digits,
        scale,
        scale_factor,
        explicit_decimal,
        sign_separate: SignSeparate::None,
    })
}

fn pic_symbol_count(text: &str, symbol: char) -> Option<usize> {
    read_symbol_count(text.as_bytes(), 0, symbol as u8)
        .ok()
        .map(|(count, _)| count)
}

fn read_symbol_count(bytes: &[u8], index: usize, symbol: u8) -> PyResult<(usize, usize)> {
    if index >= bytes.len() || bytes[index] != symbol {
        return Err(PyValueError::new_err("invalid PIC repeat"));
    }
    if index + 1 < bytes.len() && bytes[index + 1] == b'(' {
        let close = bytes[index + 2..]
            .iter()
            .position(|value| *value == b')')
            .ok_or_else(|| PyValueError::new_err("unterminated PIC repeat"))?;
        let digits = std::str::from_utf8(&bytes[index + 2..index + 2 + close])
            .map_err(|_| PyValueError::new_err("invalid PIC repeat"))?;
        let count: usize = digits
            .parse()
            .map_err(|_| PyValueError::new_err("invalid PIC repeat"))?;
        return Ok((count, index + 3 + close));
    }
    let mut count = 0usize;
    let mut cursor = index;
    while cursor < bytes.len() && bytes[cursor] == symbol {
        count += 1;
        cursor += 1;
    }
    Ok((count, cursor))
}

fn storage_size(pic: &Pic, usage: &Usage) -> usize {
    match (pic, usage) {
        (_, Usage::Comp1) | (Pic::Float { bits: 32 }, _) => 4,
        (_, Usage::Comp2) | (Pic::Float { bits: 64 }, _) => 8,
        (Pic::Alpha { len }, _) => *len,
        (
            Pic::Numeric {
                int_digits,
                scale,
                explicit_decimal,
                sign_separate,
                ..
            },
            Usage::Display,
        ) => {
            int_digits
                + scale
                + usize::from(*explicit_decimal)
                + usize::from(*sign_separate != SignSeparate::None)
        }
        (
            Pic::Numeric {
                int_digits, scale, ..
            },
            Usage::Comp3,
        ) => (int_digits + scale) / 2 + 1,
        (
            Pic::Numeric {
                int_digits, scale, ..
            },
            Usage::Comp3U,
        ) => (int_digits + scale).div_ceil(2),
        (
            Pic::Numeric {
                int_digits, scale, ..
            },
            Usage::Comp9,
        ) => match int_digits + scale {
            1..=2 => 1,
            precision => binary_storage_size(precision),
        },
        (
            Pic::Numeric {
                int_digits, scale, ..
            },
            Usage::Comp,
        ) => binary_storage_size(int_digits + scale),
        _ => 0,
    }
}

fn binary_storage_size(precision: usize) -> usize {
    match precision {
        0..=4 => 2,
        5..=9 => 4,
        10..=18 => 8,
        _ => (((std::f64::consts::LOG2_10 * precision as f64) + 1.0) / 8.0).ceil() as usize,
    }
}
