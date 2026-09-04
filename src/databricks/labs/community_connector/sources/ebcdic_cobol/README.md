# EBCDIC COBOL Lakeflow Community Connector

This connector incrementally ingests immutable EBCDIC/COBOL files from Unity
Catalog Volumes into Delta tables. It uses a Rust/PyO3 decoder on Spark workers
and implements `LakeflowConnect` plus `SupportsPartitionedStream`.

## Runtime model

- one Spark input partition per source file;
- deterministic `(mtime_ns, path)` streaming offsets;
- copybooks compiled and cached once per worker process;
- memory-mapped uncompressed files with bounded native decode batches;
- gzip files supported with bounded output batches;
- append-only Lakeflow ingestion;
- optional source file, mtime, and record-index metadata.

Input files are treated as immutable. Publish a new path instead of overwriting
an existing file, and do not backdate file modification times.

## Manifest

Create a JSON manifest on a UC Volume:

```json
{
  "tables": {
    "customers": {
      "data_path": "/Volumes/main/ebcdic/landing/customers",
      "copybook_path": "/Volumes/main/ebcdic/copybooks/customers.cpy",
      "copybook_library_path": "/Volumes/main/ebcdic/copybooks/includes",
      "file_glob": "*.dat",
      "recursive": false,
      "record_format": "F",
      "encoding": "EBCDIC",
      "batch_rows": 8192,
      "max_files_per_batch": 1000,
      "include_file_metadata": true,
      "variable_size_occurs": false
    }
  }
}
```

Use `record_format` `F`, `V`, or `VB`. Set `variable_size_occurs` to `true`
for concatenated variable-size `F` records containing `OCCURS DEPENDING ON`.

Supported runtime decoder options:

- `encoding`: `EBCDIC`, `ASCII`, `UTF16`, `HEX`, or `RAW`;
- `string_trimming_policy`: `none`, `left`, `right`, `both`, or `keep_all`;
- `utf16_big_endian`;
- `floating_point_format`: `IBM`, `IBM_LE`, `IEEE754`, or `IEEE754_LE`;
- `strict_sign_overpunch`;
- `improved_null_detection`;
- `strict_integral_precision`;
- `display_pic_as_string`;
- `null_on_error`.

## Connection

Create the community connector connection with:

```json
{
  "config_path": "/Volumes/main/ebcdic/config/config.json"
}
```

The pipeline identity needs `READ VOLUME` on every referenced Volume.

## Pipeline object

```json
{
  "table": {
    "source_table": "customers",
    "destination_table": "customers_raw",
    "table_configuration": {
      "ingestion_mode": "append",
      "max_files_per_batch": "500"
    }
  }
}
```

## Native wheel

The connector and native decoder are separate wheels. Install the connector
wheel plus the matching native wheel in the serverless pipeline environment.
Publish both ABI3 manylinux wheels and use environment markers:

```text
ebcdic_rust_canary-0.1.0-cp311-abi3-manylinux2014_x86_64.whl ; platform_machine == "x86_64"
ebcdic_rust_canary-0.1.0-cp311-abi3-manylinux2014_aarch64.whl ; platform_machine == "aarch64"
```

The generated community-connector Python source imports
`ebcdic_rust_canary` at runtime; it does not embed platform-specific native
code.

## Output metadata

When `include_file_metadata` is enabled, each row includes:

- `__source_file`;
- `__source_mtime_ns`;
- `__record_index`.

These columns provide stable lineage and can form a downstream uniqueness key.
