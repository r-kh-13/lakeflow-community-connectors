use std::fs::File;
use std::path::Path;

use memmap2::{Mmap, MmapOptions};
use pyo3::exceptions::{PyIOError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};

use crate::copybook::Layout;
use crate::decode::{
    decode_layout, decode_layout_tuple_with_consumed, decode_layout_with_consumed,
};
use crate::options::DecodeOptions;

pub type SchemaField = (String, String, usize, usize, usize);

enum InputData {
    Python(Py<PyBytes>),
    Mapped(Mmap),
    Owned(Vec<u8>),
}

#[derive(Clone, Copy)]
enum RecordFormat {
    Fixed,
    Variable,
    VariableBlocked,
}

impl RecordFormat {
    fn parse(value: &str) -> PyResult<Self> {
        match value.to_ascii_uppercase().as_str() {
            "F" => Ok(Self::Fixed),
            "V" => Ok(Self::Variable),
            "VB" => Ok(Self::VariableBlocked),
            _ => Err(PyValueError::new_err(format!(
                "unsupported record_format: {value}"
            ))),
        }
    }
}

#[derive(Clone, Copy)]
enum RowFormat {
    Dict,
    Tuple,
}

impl RowFormat {
    fn parse(value: &str) -> PyResult<Self> {
        match value.to_ascii_lowercase().as_str() {
            "dict" => Ok(Self::Dict),
            "tuple" => Ok(Self::Tuple),
            _ => Err(PyValueError::new_err(format!(
                "unsupported row_format: {value}; expected dict or tuple"
            ))),
        }
    }
}

#[pyclass(module = "lakeflow_ebcdic_decoder")]
#[derive(Clone)]
pub struct CompiledCopybook {
    pub(crate) layout: Layout,
    pub(crate) options: DecodeOptions,
}

impl CompiledCopybook {
    pub fn new(layout: Layout, options: DecodeOptions) -> Self {
        Self { layout, options }
    }
}

#[pymethods]
impl CompiledCopybook {
    #[getter]
    fn record_size(&self) -> usize {
        self.layout.record_size
    }

    fn schema(&self) -> Vec<SchemaField> {
        self.layout
            .nodes
            .iter()
            .filter(|node| !node.name.eq_ignore_ascii_case("FILLER"))
            .map(|node| {
                (
                    node.name.clone(),
                    node.spark_type(&self.options),
                    node.offset,
                    node.size,
                    node.occurs_max,
                )
            })
            .collect()
    }

    #[pyo3(signature = (
        data,
        record_format = "F",
        batch_size = 8192,
        variable_size_occurs = false,
        row_format = "tuple"
    ))]
    fn iter_batches(
        &self,
        py: Python<'_>,
        data: Py<PyBytes>,
        record_format: &str,
        batch_size: usize,
        variable_size_occurs: bool,
        row_format: &str,
    ) -> PyResult<Py<RecordBatchIterator>> {
        self.build_iterator(
            py,
            InputData::Python(data),
            record_format,
            batch_size,
            variable_size_occurs,
            row_format,
        )
    }

    #[pyo3(signature = (
        path,
        record_format = "F",
        batch_size = 8192,
        variable_size_occurs = false,
        row_format = "tuple"
    ))]
    fn iter_file_batches(
        &self,
        py: Python<'_>,
        path: &str,
        record_format: &str,
        batch_size: usize,
        variable_size_occurs: bool,
        row_format: &str,
    ) -> PyResult<Py<RecordBatchIterator>> {
        let file = File::open(Path::new(path)).map_err(|error| {
            PyIOError::new_err(format!("cannot open EBCDIC file {path}: {error}"))
        })?;
        let input = if file
            .metadata()
            .map_err(|error| PyIOError::new_err(error.to_string()))?
            .len()
            == 0
        {
            InputData::Owned(Vec::new())
        } else {
            // SAFETY: the mapping is read-only and input files are immutable.
            match unsafe { MmapOptions::new().map(&file) } {
                Ok(mapped) => InputData::Mapped(mapped),
                Err(_) => InputData::Owned(std::fs::read(path).map_err(|error| {
                    PyIOError::new_err(format!("cannot read EBCDIC file {path}: {error}"))
                })?),
            }
        };
        self.build_iterator(
            py,
            input,
            record_format,
            batch_size,
            variable_size_occurs,
            row_format,
        )
    }
}

impl CompiledCopybook {
    fn build_iterator(
        &self,
        py: Python<'_>,
        input: InputData,
        record_format: &str,
        batch_size: usize,
        variable_size_occurs: bool,
        row_format: &str,
    ) -> PyResult<Py<RecordBatchIterator>> {
        if batch_size == 0 {
            return Err(PyValueError::new_err(
                "batch_size must be greater than zero",
            ));
        }
        Py::new(
            py,
            RecordBatchIterator {
                input,
                layout: self.layout.clone(),
                options: self.options.clone(),
                format: RecordFormat::parse(record_format)?,
                batch_size,
                variable_size_occurs,
                row_format: RowFormat::parse(row_format)?,
                offset: 0,
                block_end: None,
            },
        )
    }
}

#[pyclass(module = "lakeflow_ebcdic_decoder")]
pub struct RecordBatchIterator {
    input: InputData,
    layout: Layout,
    options: DecodeOptions,
    format: RecordFormat,
    batch_size: usize,
    variable_size_occurs: bool,
    row_format: RowFormat,
    offset: usize,
    block_end: Option<usize>,
}

#[pymethods]
impl RecordBatchIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__<'py>(&mut self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyList>>> {
        let python_owner = match &self.input {
            InputData::Python(value) => Some(value.clone_ref(py)),
            _ => None,
        };
        let data: &[u8] = match (&self.input, python_owner.as_ref()) {
            (InputData::Python(_), Some(value)) => value.bind(py).as_bytes(),
            (InputData::Mapped(value), _) => value,
            (InputData::Owned(value), _) => value,
            (InputData::Python(_), None) => unreachable!(),
        };
        if self.offset >= data.len() {
            return Ok(None);
        }
        let batch = PyList::empty(py);
        for _ in 0..self.batch_size {
            if self.offset >= data.len() {
                break;
            }
            let row = match self.format {
                RecordFormat::Fixed if self.variable_size_occurs => {
                    let (row, consumed) = decode_with_consumed(
                        py,
                        &self.layout,
                        &data[self.offset..],
                        &self.options,
                        self.row_format,
                    )?;
                    if consumed == 0 {
                        return Err(PyValueError::new_err(format!(
                            "record at offset {} consumed zero bytes",
                            self.offset
                        )));
                    }
                    self.offset += consumed;
                    row
                }
                RecordFormat::Fixed => {
                    let end = self.offset.saturating_add(self.layout.record_size);
                    if end > data.len() && !self.options.null_on_error {
                        return Err(PyValueError::new_err(format!(
                            "truncated fixed-length file at offset {}",
                            self.offset
                        )));
                    }
                    let end = end.min(data.len());
                    let row = decode_value(
                        py,
                        &self.layout,
                        &data[self.offset..end],
                        &self.options,
                        self.row_format,
                    )?;
                    self.offset = end;
                    row
                }
                RecordFormat::Variable => {
                    let (start, end, next) = next_rdw(data, self.offset, data.len())?;
                    let row = decode_value(
                        py,
                        &self.layout,
                        &data[start..end],
                        &self.options,
                        self.row_format,
                    )?;
                    self.offset = next;
                    row
                }
                RecordFormat::VariableBlocked => {
                    let (start, end, next) = next_vb(data, &mut self.offset, &mut self.block_end)?;
                    let row = decode_value(
                        py,
                        &self.layout,
                        &data[start..end],
                        &self.options,
                        self.row_format,
                    )?;
                    self.offset = next;
                    row
                }
            };
            batch.append(row)?;
        }
        Ok((!batch.is_empty()).then_some(batch))
    }
}

fn decode_value<'py>(
    py: Python<'py>,
    layout: &Layout,
    record: &[u8],
    options: &DecodeOptions,
    row_format: RowFormat,
) -> PyResult<Bound<'py, PyAny>> {
    match row_format {
        RowFormat::Dict => Ok(decode_layout(py, layout, record, options)?.into_any()),
        RowFormat::Tuple => Ok(
            decode_layout_tuple_with_consumed(py, layout, record, options)?
                .0
                .into_any(),
        ),
    }
}

fn decode_with_consumed<'py>(
    py: Python<'py>,
    layout: &Layout,
    record: &[u8],
    options: &DecodeOptions,
    row_format: RowFormat,
) -> PyResult<(Bound<'py, PyAny>, usize)> {
    match row_format {
        RowFormat::Dict => {
            let (row, consumed) = decode_layout_with_consumed(py, layout, record, options)?;
            Ok((row.into_any(), consumed))
        }
        RowFormat::Tuple => {
            let (row, consumed) = decode_layout_tuple_with_consumed(py, layout, record, options)?;
            Ok((row.into_any(), consumed))
        }
    }
}

fn next_vb(
    data: &[u8],
    offset: &mut usize,
    block_end: &mut Option<usize>,
) -> PyResult<(usize, usize, usize)> {
    if *block_end == Some(*offset) {
        *block_end = None;
    }
    if block_end.is_none() {
        if *offset + 4 > data.len() {
            return Err(PyValueError::new_err(format!(
                "truncated BDW at offset {}",
                *offset
            )));
        }
        let block_len = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as usize;
        if block_len < 4 {
            return Err(PyValueError::new_err("invalid BDW length"));
        }
        let next_block_end = *offset + block_len;
        if next_block_end > data.len() {
            return Err(PyValueError::new_err("BDW length exceeds file"));
        }
        *block_end = Some(next_block_end);
        *offset += 4;
    }
    next_rdw(data, *offset, block_end.expect("block end initialized"))
}

fn next_rdw(data: &[u8], offset: usize, limit: usize) -> PyResult<(usize, usize, usize)> {
    if offset + 4 > limit {
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
    if end > limit {
        return Err(PyValueError::new_err(format!(
            "RDW length {payload_len} exceeds remaining bytes at offset {offset}"
        )));
    }
    Ok((start, end, end))
}
