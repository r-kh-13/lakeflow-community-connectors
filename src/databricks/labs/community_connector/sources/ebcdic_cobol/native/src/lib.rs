use std::collections::HashMap;

mod compiled;
mod copybook;
mod decode;
mod ebcdic;
mod float;
mod framing;
mod options;
mod preprocess;
mod structure;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyList;

use compiled::{CompiledCopybook, RecordBatchIterator};
use copybook::parse_copybook;
use decode::{decode_layout, decode_layout_with_consumed};
use framing::split_records;
use options::DecodeOptions;

type SchemaField = (String, String, usize, usize, usize);

#[allow(clippy::too_many_arguments)]
fn parse_options(
    null_on_error: bool,
    encoding: &str,
    string_trimming_policy: &str,
    utf16_big_endian: bool,
    floating_point_format: &str,
    strict_sign_overpunch: bool,
    improved_null_detection: bool,
    strict_integral_precision: bool,
    display_pic_as_string: bool,
) -> PyResult<DecodeOptions> {
    DecodeOptions::parse(
        encoding,
        string_trimming_policy,
        utf16_big_endian,
        floating_point_format,
        strict_sign_overpunch,
        improved_null_detection,
        strict_integral_precision,
        display_pic_as_string,
        null_on_error,
    )
}

#[pyfunction(signature = (
    copybook,
    null_on_error = false,
    encoding = "ebcdic",
    string_trimming_policy = "right",
    utf16_big_endian = true,
    floating_point_format = "ibm",
    strict_sign_overpunch = false,
    improved_null_detection = false,
    strict_integral_precision = false,
    display_pic_as_string = false,
    copybooks = None
))]
#[allow(clippy::too_many_arguments)]
fn compile_copybook(
    copybook: &str,
    null_on_error: bool,
    encoding: &str,
    string_trimming_policy: &str,
    utf16_big_endian: bool,
    floating_point_format: &str,
    strict_sign_overpunch: bool,
    improved_null_detection: bool,
    strict_integral_precision: bool,
    display_pic_as_string: bool,
    copybooks: Option<HashMap<String, String>>,
) -> PyResult<CompiledCopybook> {
    let options = parse_options(
        null_on_error,
        encoding,
        string_trimming_policy,
        utf16_big_endian,
        floating_point_format,
        strict_sign_overpunch,
        improved_null_detection,
        strict_integral_precision,
        display_pic_as_string,
    )?;
    let expanded =
        preprocess::expand_copybook(copybook, copybooks.as_ref().unwrap_or(&HashMap::new()))?;
    Ok(CompiledCopybook::new(parse_copybook(&expanded)?, options))
}

#[pyfunction(signature = (
    data,
    copybook,
    record_format = "F",
    null_on_error = false,
    encoding = "ebcdic",
    string_trimming_policy = "right",
    utf16_big_endian = true,
    floating_point_format = "ibm",
    strict_sign_overpunch = false,
    improved_null_detection = false,
    strict_integral_precision = false,
    display_pic_as_string = false,
    variable_size_occurs = false,
    copybooks = None
))]
#[allow(clippy::too_many_arguments)]
fn decode_records<'py>(
    py: Python<'py>,
    data: &[u8],
    copybook: &str,
    record_format: &str,
    null_on_error: bool,
    encoding: &str,
    string_trimming_policy: &str,
    utf16_big_endian: bool,
    floating_point_format: &str,
    strict_sign_overpunch: bool,
    improved_null_detection: bool,
    strict_integral_precision: bool,
    display_pic_as_string: bool,
    variable_size_occurs: bool,
    copybooks: Option<HashMap<String, String>>,
) -> PyResult<Bound<'py, PyList>> {
    let decoder = compile_copybook(
        copybook,
        null_on_error,
        encoding,
        string_trimming_policy,
        utf16_big_endian,
        floating_point_format,
        strict_sign_overpunch,
        improved_null_detection,
        strict_integral_precision,
        display_pic_as_string,
        copybooks,
    )?;
    decode_with_layout(
        py,
        data,
        &decoder.layout,
        record_format,
        &decoder.options,
        variable_size_occurs,
    )
}

fn decode_with_layout<'py>(
    py: Python<'py>,
    data: &[u8],
    layout: &copybook::Layout,
    record_format: &str,
    options: &DecodeOptions,
    variable_size_occurs: bool,
) -> PyResult<Bound<'py, PyList>> {
    let rows = PyList::empty(py);
    if variable_size_occurs && record_format.eq_ignore_ascii_case("F") {
        let mut offset = 0usize;
        while offset < data.len() {
            let (row, consumed) =
                decode_layout_with_consumed(py, layout, &data[offset..], options)?;
            if consumed == 0 {
                return Err(PyValueError::new_err(format!(
                    "record at offset {offset} consumed zero bytes"
                )));
            }
            rows.append(row)?;
            offset += consumed;
        }
        return Ok(rows);
    }
    for record in split_records(
        data,
        record_format,
        layout.record_size,
        options.null_on_error,
    )? {
        rows.append(decode_layout(py, layout, record, options)?)?;
    }
    Ok(rows)
}

#[pyfunction(signature = (
    copybook,
    encoding = "ebcdic",
    strict_integral_precision = false,
    display_pic_as_string = false,
    copybooks = None
))]
fn copybook_schema(
    copybook: &str,
    encoding: &str,
    strict_integral_precision: bool,
    display_pic_as_string: bool,
    copybooks: Option<HashMap<String, String>>,
) -> PyResult<Vec<SchemaField>> {
    let decoder = compile_copybook(
        copybook,
        false,
        encoding,
        "right",
        true,
        "ibm",
        false,
        false,
        strict_integral_precision,
        display_pic_as_string,
        copybooks,
    )?;
    Ok(decoder
        .layout
        .nodes
        .iter()
        .filter(|node| !node.name.eq_ignore_ascii_case("FILLER"))
        .map(|node| {
            (
                node.name.clone(),
                node.spark_type(&decoder.options),
                node.offset,
                node.size,
                node.occurs_max,
            )
        })
        .collect())
}

#[pyfunction]
fn expand_copybook_source(copybook: &str, copybooks: HashMap<String, String>) -> PyResult<String> {
    preprocess::expand_copybook(copybook, &copybooks)
}

#[pymodule]
fn ebcdic_rust_canary(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<CompiledCopybook>()?;
    module.add_class::<RecordBatchIterator>()?;
    module.add_function(wrap_pyfunction!(compile_copybook, module)?)?;
    module.add_function(wrap_pyfunction!(copybook_schema, module)?)?;
    module.add_function(wrap_pyfunction!(decode_records, module)?)?;
    module.add_function(wrap_pyfunction!(expand_copybook_source, module)?)?;
    Ok(())
}
