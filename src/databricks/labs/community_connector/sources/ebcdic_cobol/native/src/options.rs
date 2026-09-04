use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Ebcdic,
    Ascii,
    Utf16,
    Hex,
    Raw,
}

impl TextEncoding {
    pub fn parse(value: &str) -> PyResult<Self> {
        match normalize(value).as_str() {
            "ebcdic" => Ok(Self::Ebcdic),
            "ascii" => Ok(Self::Ascii),
            "utf16" => Ok(Self::Utf16),
            "hex" => Ok(Self::Hex),
            "raw" | "binary" => Ok(Self::Raw),
            _ => Err(PyValueError::new_err(format!(
                "unsupported encoding: {value}; expected EBCDIC, ASCII, UTF16, HEX, or RAW"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringTrim {
    None,
    Left,
    Right,
    Both,
    KeepAll,
}

impl StringTrim {
    pub fn parse(value: &str) -> PyResult<Self> {
        match normalize(value).as_str() {
            "none" | "trimnone" => Ok(Self::None),
            "left" | "trimleft" => Ok(Self::Left),
            "right" | "trimright" => Ok(Self::Right),
            "both" | "trim" | "trimboth" => Ok(Self::Both),
            "keepall" => Ok(Self::KeepAll),
            _ => Err(PyValueError::new_err(format!(
                "unsupported string_trimming_policy: {value}"
            ))),
        }
    }

    pub fn apply(self, value: String) -> String {
        match self {
            Self::None | Self::KeepAll => value,
            Self::Left => value.trim_start_matches(|ch: char| ch <= ' ').to_string(),
            Self::Right => value.trim_end_matches(|ch: char| ch <= ' ').to_string(),
            Self::Both => value.trim_matches(|ch: char| ch <= ' ').to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatingPointFormat {
    Ibm,
    IbmLittleEndian,
    Ieee754,
    Ieee754LittleEndian,
}

impl FloatingPointFormat {
    pub fn parse(value: &str) -> PyResult<Self> {
        match normalize(value).as_str() {
            "ibm" | "ibmbe" => Ok(Self::Ibm),
            "ibmle" => Ok(Self::IbmLittleEndian),
            "ieee754" | "ieee754be" | "ieee" => Ok(Self::Ieee754),
            "ieee754le" | "ieeele" => Ok(Self::Ieee754LittleEndian),
            _ => Err(PyValueError::new_err(format!(
                "unsupported floating_point_format: {value}"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DecodeOptions {
    pub text_encoding: TextEncoding,
    pub string_trim: StringTrim,
    pub utf16_big_endian: bool,
    pub floating_point_format: FloatingPointFormat,
    pub strict_sign_overpunch: bool,
    pub improved_null_detection: bool,
    pub strict_integral_precision: bool,
    pub display_pic_as_string: bool,
    pub null_on_error: bool,
}

impl DecodeOptions {
    #[allow(clippy::too_many_arguments)]
    pub fn parse(
        encoding: &str,
        string_trimming_policy: &str,
        utf16_big_endian: bool,
        floating_point_format: &str,
        strict_sign_overpunch: bool,
        improved_null_detection: bool,
        strict_integral_precision: bool,
        display_pic_as_string: bool,
        null_on_error: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            text_encoding: TextEncoding::parse(encoding)?,
            string_trim: StringTrim::parse(string_trimming_policy)?,
            utf16_big_endian,
            floating_point_format: FloatingPointFormat::parse(floating_point_format)?,
            strict_sign_overpunch,
            improved_null_detection,
            strict_integral_precision,
            display_pic_as_string,
            null_on_error,
        })
    }
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}
