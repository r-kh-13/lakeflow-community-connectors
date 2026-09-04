"""Lakeflow community connector for EBCDIC/COBOL files on UC Volumes."""

from __future__ import annotations

import gzip
import json
import os
from fnmatch import fnmatch
from itertools import chain
from pathlib import Path
from typing import Iterator, Sequence

from pyspark.sql.types import (
    ArrayType,
    BinaryType,
    DataType,
    DecimalType,
    DoubleType,
    FloatType,
    IntegerType,
    LongType,
    StringType,
    StructField,
    StructType,
)

from databricks.labs.community_connector.interface import (
    LakeflowConnect,
    SupportsPartitionedStream,
)

_DECODER_OPTION_NAMES = (
    "encoding",
    "string_trimming_policy",
    "utf16_big_endian",
    "floating_point_format",
    "strict_sign_overpunch",
    "improved_null_detection",
    "strict_integral_precision",
    "display_pic_as_string",
    "null_on_error",
)
_BOOLEAN_OPTIONS = {
    "utf16_big_endian",
    "strict_sign_overpunch",
    "improved_null_detection",
    "strict_integral_precision",
    "display_pic_as_string",
    "null_on_error",
    "variable_size_occurs",
    "recursive",
    "include_file_metadata",
}
_COPYBOOK_SUFFIXES = {".cob", ".copybook", ".cpy"}
_DEFAULT_BATCH_ROWS = 8192
_DEFAULT_MAX_FILES_PER_BATCH = 1000
_TEXT_CACHE: dict[tuple[str, int], str] = {}


class EbcdicCobolLakeflowConnect(LakeflowConnect, SupportsPartitionedStream):
    """Read immutable EBCDIC files using copybooks and a native Rust decoder.

    Connection options:
        config_path: Absolute path to a JSON manifest on a UC Volume.
        config_json: Inline JSON manifest; intended primarily for tests.

    The manifest contains ``{"tables": {"name": {...}}}``. Each table requires
    ``data_path`` and ``copybook_path`` and may override decoder/framing options.
    """

    def __init__(self, options: dict[str, str]) -> None:
        super().__init__(options)
        self._manifest = _load_manifest(options)
        raw_tables = self._manifest.get("tables")
        if not isinstance(raw_tables, dict) or not raw_tables:
            raise ValueError("EBCDIC manifest must contain a non-empty 'tables' object")
        self._tables: dict[str, dict] = {}
        self._decoder_cache: dict[tuple, object] = {}
        for raw_name, raw_config in raw_tables.items():
            name = str(raw_name).strip()
            if not name or not isinstance(raw_config, dict):
                raise ValueError("Each EBCDIC table must have a name and object configuration")
            config = dict(raw_config)
            _required_path(config, "data_path", name)
            _required_path(config, "copybook_path", name)
            self._tables[name] = config

    def list_tables(self) -> list[str]:
        return sorted(self._tables)

    def get_table_schema(self, table_name: str, table_options: dict[str, str]) -> StructType:
        config = self._table_config(table_name, table_options)
        declared_schema = config.get("schema")
        if declared_schema is not None:
            fields = _parse_declared_schema(declared_schema)
        else:
            decoder = self._compiled_decoder(config)
            fields = [
                StructField(name, _parse_spark_type(data_type), nullable=True)
                for name, data_type, _, _, _ in decoder.schema()
            ]
        if _bool_option(config, "include_file_metadata", True):
            fields.extend(
                [
                    StructField("__source_file", StringType(), nullable=False),
                    StructField("__source_mtime_ns", LongType(), nullable=False),
                    StructField("__record_index", LongType(), nullable=False),
                ]
            )
        return StructType(fields)

    def read_table_metadata(self, table_name: str, table_options: dict[str, str]) -> dict:
        self._table_config(table_name, table_options)
        return {
            "primary_keys": None,
            "cursor_field": None,
            "ingestion_type": "append",
        }

    def read_table(
        self,
        table_name: str,
        start_offset: dict | None,
        table_options: dict[str, str],
    ) -> tuple[Iterator[dict], dict]:
        end_offset = self.latest_offset(table_name, table_options, start_offset)
        partitions = self.get_partitions(
            table_name,
            table_options,
            start_offset,
            end_offset,
        )
        records = chain.from_iterable(
            self.read_partition(table_name, partition, table_options) for partition in partitions
        )
        return records, end_offset

    def latest_offset(
        self,
        table_name: str,
        table_options: dict[str, str],
        start_offset: dict | None = None,
    ) -> dict:
        config = self._table_config(table_name, table_options)
        start = _offset_key(start_offset)
        pending = [entry for entry in _discover_files(config) if entry[:2] > start]
        max_files = _positive_int_option(
            config,
            "max_files_per_batch",
            _DEFAULT_MAX_FILES_PER_BATCH,
        )
        pending = pending[:max_files]
        if not pending:
            return _canonical_offset(start_offset)
        mtime_ns, path, _ = pending[-1]
        return {"mtime_ns": mtime_ns, "path": path}

    def get_partitions(
        self,
        table_name: str,
        table_options: dict[str, str],
        start_offset: dict | None = None,
        end_offset: dict | None = None,
    ) -> Sequence[dict]:
        config = self._table_config(table_name, table_options)
        files = _discover_files(config)
        if start_offset is None and end_offset is None:
            selected = files
        else:
            start = _offset_key(start_offset)
            end = _offset_key(end_offset)
            if start == end:
                return []
            selected = [entry for entry in files if start < entry[:2] <= end]
        return [
            {"mtime_ns": mtime_ns, "path": path, "size": size} for mtime_ns, path, size in selected
        ]

    def read_partition(
        self,
        table_name: str,
        partition: dict,
        table_options: dict[str, str],
    ) -> Iterator[dict]:
        # pylint: disable=too-many-locals
        config = self._table_config(table_name, table_options)
        path = str(partition["path"])
        decoder = self._compiled_decoder(config)
        record_format = str(config.get("record_format", "F"))
        batch_rows = _positive_int_option(
            config,
            "batch_rows",
            _DEFAULT_BATCH_ROWS,
        )
        variable_size_occurs = _bool_option(config, "variable_size_occurs", False)
        include_metadata = _bool_option(config, "include_file_metadata", True)
        mtime_ns = int(partition["mtime_ns"])

        if path.lower().endswith(".gz"):
            with gzip.open(path, "rb") as handle:
                batches = decoder.iter_batches(
                    handle.read(),
                    record_format=record_format,
                    batch_size=batch_rows,
                    variable_size_occurs=variable_size_occurs,
                    row_format="dict",
                )
        else:
            batches = decoder.iter_file_batches(
                path,
                record_format=record_format,
                batch_size=batch_rows,
                variable_size_occurs=variable_size_occurs,
                row_format="dict",
            )

        record_index = 0
        for batch in batches:
            for row in batch:
                if include_metadata:
                    row["__source_file"] = path
                    row["__source_mtime_ns"] = mtime_ns
                    row["__record_index"] = record_index
                record_index += 1
                yield row

    def _table_config(
        self,
        table_name: str,
        table_options: dict[str, str] | None,
    ) -> dict:
        if table_name not in self._tables:
            raise ValueError(
                f"Unknown EBCDIC table {table_name!r}; expected one of {sorted(self._tables)}"
            )
        config = dict(self._tables[table_name])
        for key, value in (table_options or {}).items():
            if key in _DECODER_OPTION_NAMES or key in {
                "batch_rows",
                "file_glob",
                "max_files_per_batch",
                "record_format",
                "recursive",
                "variable_size_occurs",
                "include_file_metadata",
            }:
                config[key] = value
        return config

    def _compiled_decoder(self, config: dict):
        definition = _decoder_definition(config)
        if definition not in self._decoder_cache:
            self._decoder_cache[definition] = _compile_decoder(*definition)
        return self._decoder_cache[definition]

    def __getstate__(self) -> dict:
        """Drop native objects when Spark serializes the connector."""
        state = self.__dict__.copy()
        state["_decoder_cache"] = {}
        return state


def _load_manifest(options: dict[str, str]) -> dict:
    inline = options.get("config_json")
    path = options.get("config_path")
    if bool(inline) == bool(path):
        raise ValueError("Provide exactly one of 'config_path' or 'config_json'")
    if inline:
        try:
            result = json.loads(inline)
        except json.JSONDecodeError as error:
            raise ValueError(f"Invalid config_json: {error}") from error
    else:
        config_path = os.path.abspath(str(path))
        with open(config_path, encoding="utf-8") as handle:
            result = json.load(handle)
    if not isinstance(result, dict):
        raise ValueError("EBCDIC manifest must be a JSON object")
    return result


def _required_path(config: dict, option: str, table_name: str) -> str:
    value = str(config.get(option, "")).strip()
    if not value or not os.path.isabs(value):
        raise ValueError(f"Table {table_name!r} requires absolute {option}, got {value!r}")
    return value


def _bool_option(config: dict, name: str, default: bool) -> bool:
    value = config.get(name)
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    normalized = str(value).strip().lower()
    if normalized in {"1", "true", "yes", "y", "on"}:
        return True
    if normalized in {"0", "false", "no", "n", "off"}:
        return False
    raise ValueError(f"{name} must be true or false, got {value!r}")


def _positive_int_option(config: dict, name: str, default: int) -> int:
    value = int(config.get(name, default))
    if value <= 0:
        raise ValueError(f"{name} must be greater than zero")
    return value


def _discover_files(config: dict) -> list[tuple[int, str, int]]:
    root = Path(str(config["data_path"]))
    if not root.is_dir():
        raise FileNotFoundError(f"EBCDIC data_path does not exist: {root}")
    pattern = str(config.get("file_glob", "*"))
    recursive = _bool_option(config, "recursive", False)
    candidates = root.rglob("*") if recursive else root.iterdir()
    files = []
    for candidate in candidates:
        if candidate.is_file() and fnmatch(candidate.name, pattern):
            stat = candidate.stat()
            files.append((stat.st_mtime_ns, str(candidate), stat.st_size))
    return sorted(files)


def _offset_key(offset: dict | None) -> tuple[int, str]:
    if not offset:
        return (-1, "")
    return (int(offset.get("mtime_ns", -1)), str(offset.get("path", "")))


def _canonical_offset(offset: dict | None) -> dict:
    mtime_ns, path = _offset_key(offset)
    return {"mtime_ns": mtime_ns, "path": path}


def _load_copybook_bundle(config: dict) -> tuple[str, dict[str, str]]:
    copybook_path = Path(str(config["copybook_path"]))
    source = _read_text_cached(str(copybook_path), copybook_path.stat().st_mtime_ns)
    library_path = config.get("copybook_library_path")
    if not library_path:
        return source, {}
    root = Path(str(library_path))
    if not root.is_dir():
        raise FileNotFoundError(f"copybook_library_path does not exist: {root}")
    library = {}
    for candidate in sorted(root.rglob("*")):
        if candidate.is_file() and candidate.suffix.lower() in _COPYBOOK_SUFFIXES:
            library[candidate.name] = _read_text_cached(
                str(candidate),
                candidate.stat().st_mtime_ns,
            )
    return source, library


def _read_text_cached(path: str, _mtime_ns: int) -> str:
    key = (path, _mtime_ns)
    if key not in _TEXT_CACHE:
        _bounded_cache_put(
            _TEXT_CACHE,
            key,
            Path(path).read_text(encoding="utf-8"),
            128,
        )
    return _TEXT_CACHE[key]


def _decoder_definition(config: dict) -> tuple:
    copybook, library = _load_copybook_bundle(config)
    decoder_options = tuple(
        (
            name,
            _bool_option(config, name, False) if name in _BOOLEAN_OPTIONS else str(config[name]),
        )
        for name in _DECODER_OPTION_NAMES
        if name in config
    )
    return copybook, json.dumps(library, sort_keys=True), decoder_options


def _compile_decoder(
    copybook: str,
    library_json: str,
    decoder_options: tuple[tuple[str, object], ...],
):
    # Import lazily on the executor so driver-side connector discovery can
    # still report an actionable missing-wheel error.
    # pylint: disable=import-outside-toplevel
    try:
        from ebcdic_rust_canary import compile_copybook
    except ImportError as error:
        raise RuntimeError(
            "The native ebcdic-rust-canary wheel is required on the Lakeflow "
            "pipeline environment for both x86_64 and aarch64 workers. "
            f"Native import failed with: {error!r}"
        ) from error
    kwargs = {}
    for name, value in decoder_options:
        kwargs[name] = (
            _bool_option({name: value}, name, False) if name in _BOOLEAN_OPTIONS else value
        )
    decoder = compile_copybook(
        copybook,
        copybooks=json.loads(library_json),
        **kwargs,
    )
    return decoder


def _bounded_cache_put(cache: dict, key, value, capacity: int) -> None:
    if len(cache) >= capacity:
        cache.pop(next(iter(cache)))
    cache[key] = value


def _parse_spark_type(specification: str) -> DataType:
    spec = specification.strip()
    lowered = spec.lower()
    primitives: dict[str, DataType] = {
        "string": StringType(),
        "integer": IntegerType(),
        "long": LongType(),
        "float": FloatType(),
        "double": DoubleType(),
        "binary": BinaryType(),
    }
    if lowered in primitives:
        return primitives[lowered]
    if lowered.startswith("decimal(") and lowered.endswith(")"):
        precision, scale = lowered[8:-1].split(",", 1)
        return DecimalType(int(precision), int(scale))
    if lowered.startswith("array<") and spec.endswith(">"):
        return ArrayType(_parse_spark_type(spec[6:-1]), containsNull=True)
    if lowered.startswith("struct<") and spec.endswith(">"):
        fields = []
        for field_spec in _split_top_level(spec[7:-1]):
            name, data_type = field_spec.split(":", 1)
            fields.append(StructField(name, _parse_spark_type(data_type), nullable=True))
        return StructType(fields)
    raise ValueError(f"Unsupported native Spark type: {specification}")


def _parse_declared_schema(value) -> list[StructField]:
    if not isinstance(value, list) or not value:
        raise ValueError("Table schema must be a non-empty list")
    fields = []
    for item in value:
        if not isinstance(item, dict) or not item.get("name") or not item.get("type"):
            raise ValueError("Each table schema field requires 'name' and 'type'")
        fields.append(
            StructField(
                str(item["name"]),
                _parse_spark_type(str(item["type"])),
                nullable=bool(item.get("nullable", True)),
            )
        )
    return fields


def _split_top_level(value: str) -> list[str]:
    result = []
    start = 0
    depth = 0
    for index, character in enumerate(value):
        if character in "<(":
            depth += 1
        elif character in ">)":
            depth -= 1
        elif character == "," and depth == 0:
            result.append(value[start:index])
            start = index + 1
    result.append(value[start:])
    return result
