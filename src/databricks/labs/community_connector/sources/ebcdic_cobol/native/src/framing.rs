use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

pub fn split_records<'a>(
    data: &'a [u8],
    record_format: &str,
    record_size: usize,
    null_on_error: bool,
) -> PyResult<Vec<&'a [u8]>> {
    match record_format.to_ascii_uppercase().as_str() {
        "F" => split_fixed(data, record_size, null_on_error),
        "V" => split_rdw(data),
        "VB" => split_vb(data),
        other => Err(PyValueError::new_err(format!(
            "unsupported record_format: {other}"
        ))),
    }
}

fn split_fixed(data: &[u8], record_size: usize, null_on_error: bool) -> PyResult<Vec<&[u8]>> {
    if record_size == 0 {
        return Err(PyValueError::new_err("fixed record size cannot be zero"));
    }
    if !null_on_error && !data.len().is_multiple_of(record_size) {
        return Err(PyValueError::new_err(format!(
            "truncated fixed-length file: {} bytes is not a multiple of {record_size}",
            data.len()
        )));
    }
    Ok(data.chunks(record_size).collect())
}

fn split_rdw(data: &[u8]) -> PyResult<Vec<&[u8]>> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        if offset + 4 > data.len() {
            return Err(PyValueError::new_err(format!(
                "truncated RDW at offset {offset}"
            )));
        }
        let header = &data[offset..offset + 4];
        if header == [0, 0, 0, 0] {
            return Err(PyValueError::new_err(format!(
                "RDW headers should never be zero (0,0,0,0). Found zero size record at {offset}."
            )));
        }
        let payload_len = u16::from_be_bytes([header[0], header[1]]) as usize;
        let start = offset + 4;
        let end = start + payload_len;
        if end > data.len() {
            return Err(PyValueError::new_err(format!(
                "RDW length {payload_len} exceeds remaining bytes at offset {offset}"
            )));
        }
        records.push(&data[start..end]);
        offset = end;
    }
    Ok(records)
}

fn split_vb(data: &[u8]) -> PyResult<Vec<&[u8]>> {
    let mut records = Vec::new();
    let mut offset = 0usize;
    while offset < data.len() {
        if offset + 4 > data.len() {
            return Err(PyValueError::new_err(format!(
                "truncated BDW at offset {offset}"
            )));
        }
        let block_len = u16::from_be_bytes([data[offset], data[offset + 1]]) as usize;
        if block_len < 4 {
            return Err(PyValueError::new_err("invalid BDW length"));
        }
        let block_end = offset + block_len;
        if block_end > data.len() {
            return Err(PyValueError::new_err("BDW length exceeds file"));
        }
        records.extend(split_rdw(&data[offset + 4..block_end])?);
        offset = block_end;
    }
    Ok(records)
}
