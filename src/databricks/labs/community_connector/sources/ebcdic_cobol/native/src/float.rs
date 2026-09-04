// IBM hexadecimal floating-point conversion adapted from AbsaOSS/cobrix,
// whose implementation is based on Enthought's ibm2ieee library.
// Cobrix: Copyright 2018 ABSA Group Limited, Apache License 2.0.
// ibm2ieee: Copyright 2018 Enthought, Inc., BSD 3-Clause.
// See ../../THIRD_PARTY_NOTICES.md.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use crate::options::FloatingPointFormat;

const BIT_COUNT_MAGIC: u32 = 0x0000_55AF;

pub fn decode_single(bytes: &[u8], format: FloatingPointFormat) -> PyResult<f32> {
    match format {
        FloatingPointFormat::Ibm => decode_ibm_single_be(bytes),
        FloatingPointFormat::IbmLittleEndian => decode_ibm_single_le(bytes),
        FloatingPointFormat::Ieee754 => decode_ieee_single(bytes, false),
        FloatingPointFormat::Ieee754LittleEndian => decode_ieee_single(bytes, true),
    }
}

pub fn decode_double(bytes: &[u8], format: FloatingPointFormat) -> PyResult<f64> {
    match format {
        FloatingPointFormat::Ibm => decode_ibm_double_be(bytes),
        FloatingPointFormat::IbmLittleEndian => decode_ibm_double_le(bytes),
        FloatingPointFormat::Ieee754 => decode_ieee_double(bytes, false),
        FloatingPointFormat::Ieee754LittleEndian => decode_ieee_double(bytes, true),
    }
}

fn decode_ibm_single_be(bytes: &[u8]) -> PyResult<f32> {
    if bytes.len() != 4 {
        return Err(PyValueError::new_err("COMP-1 requires 4 bytes"));
    }
    decode_ibm_single(u32::from_be_bytes(bytes.try_into().unwrap()))
}

fn decode_ibm_single_le(bytes: &[u8]) -> PyResult<f32> {
    if bytes.len() != 4 {
        return Err(PyValueError::new_err("COMP-1 requires 4 bytes"));
    }
    decode_ibm_single(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn decode_ibm_single(mantissa: u32) -> PyResult<f32> {
    const SIGN: u32 = 0x8000_0000;
    const EXP: u32 = 0x7F00_0000;
    const FRACT: u32 = 0x00FF_FFFF;
    const MS_NIBBLE: u32 = 0x00F0_0000;

    let sign = mantissa & SIGN;
    let mut fracture = mantissa & FRACT;
    let mut exponent = ((mantissa & EXP) >> 22) as i32;
    if fracture == 0 {
        return Ok(0.0);
    }
    while fracture & MS_NIBBLE == 0 {
        fracture <<= 4;
        exponent -= 4;
    }
    let top_nibble = fracture & MS_NIBBLE;
    let leading_zeros = ((BIT_COUNT_MAGIC >> (top_nibble >> 19)) & 3) as i32;
    fracture <<= leading_zeros as u32;
    let converted_exp = exponent - 131 - leading_zeros;
    if (0..254).contains(&converted_exp) {
        Ok(f32::from_bits(
            sign + ((converted_exp as u32) << 23) + fracture,
        ))
    } else if converted_exp > 254 {
        Ok(if sign != 0 {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        })
    } else if converted_exp >= -32 {
        let mask = !(0xFFFF_FFFDu32 << (-1 - converted_exp) as u32);
        let round_up = u32::from((fracture & mask) > 0);
        let converted_fract = ((fracture >> (-1 - converted_exp) as u32) + round_up) >> 1;
        Ok(f32::from_bits(sign + converted_fract))
    } else {
        Ok(0.0)
    }
}

fn decode_ibm_double_be(bytes: &[u8]) -> PyResult<f64> {
    if bytes.len() != 8 {
        return Err(PyValueError::new_err("COMP-2 requires 8 bytes"));
    }
    decode_ibm_double(u64::from_be_bytes(bytes.try_into().unwrap()))
}

fn decode_ibm_double_le(bytes: &[u8]) -> PyResult<f64> {
    if bytes.len() != 8 {
        return Err(PyValueError::new_err("COMP-2 requires 8 bytes"));
    }
    decode_ibm_double(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn decode_ibm_double(mantissa: u64) -> PyResult<f64> {
    const SIGN: u64 = 0x8000_0000_0000_0000;
    const EXP: u64 = 0x7F00_0000_0000_0000;
    const FRACT: u64 = 0x00FF_FFFF_FFFF_FFFF;
    const MS_NIBBLE: u64 = 0x00F0_0000_0000_0000;

    let sign = mantissa & SIGN;
    let mut fracture = mantissa & FRACT;
    let mut exponent = (mantissa & EXP) >> 54;
    if fracture == 0 {
        return Ok(0.0);
    }
    while fracture & MS_NIBBLE == 0 {
        fracture <<= 4;
        exponent = exponent.wrapping_sub(4);
    }
    let top_nibble = fracture & MS_NIBBLE;
    let leading_zeros = (u64::from(BIT_COUNT_MAGIC) >> (top_nibble >> 51)) & 3;
    fracture <<= leading_zeros;
    let converted_exp = exponent + 765 - leading_zeros;
    let round_up = u64::from((fracture & 0xb) > 0);
    let converted_fract = ((fracture >> 2) + round_up) >> 1;
    Ok(f64::from_bits(
        sign + (converted_exp << 52) + converted_fract,
    ))
}

fn decode_ieee_single(bytes: &[u8], little_endian: bool) -> PyResult<f32> {
    if bytes.len() != 4 {
        return Err(PyValueError::new_err("IEEE754 single requires 4 bytes"));
    }
    let bits = if little_endian {
        u32::from_le_bytes(bytes.try_into().unwrap())
    } else {
        u32::from_be_bytes(bytes.try_into().unwrap())
    };
    Ok(f32::from_bits(bits))
}

fn decode_ieee_double(bytes: &[u8], little_endian: bool) -> PyResult<f64> {
    if bytes.len() != 8 {
        return Err(PyValueError::new_err("IEEE754 double requires 8 bytes"));
    }
    let bits = if little_endian {
        u64::from_le_bytes(bytes.try_into().unwrap())
    } else {
        u64::from_be_bytes(bytes.try_into().unwrap())
    };
    Ok(f64::from_bits(bits))
}
