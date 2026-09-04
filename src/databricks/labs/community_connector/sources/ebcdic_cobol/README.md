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
      "schema": [
        {"name": "NAME", "type": "string"},
        {"name": "CUSTOMER_ID", "type": "integer"},
        {"name": "AMOUNT", "type": "decimal(5,2)"}
      ],
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

`schema` is optional for direct Spark Data Source use, where it can be inferred
from the copybook. Declare it for managed ingestion pipelines: Databricks
performs partial analysis in an environment that may not install
architecture-specific native wheels. Runtime decoding still validates and
produces values according to the copybook.

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
    "connector_options": {
      "community_connector_options": {
        "options": {
          "max_files_per_batch": "500"
        }
      }
    }
  }
}
```

## Native wheel

The connector and native decoder are separate wheels. Install the connector
wheel plus the ABI3 manylinux wheel matching the pipeline architecture. Publish
both architecture builds:

```text
ebcdic_rust_canary-0.1.0-cp311-abi3-manylinux2014_x86_64.whl
ebcdic_rust_canary-0.1.0-cp311-abi3-manylinux2014_aarch64.whl
```

Do not append a PEP 508 marker directly to a Volume wheel path in an SDP
`environment.dependencies` entry. The environment installer passes that entry
to IPython `%pip`, where the semicolon is interpreted by the shell. The
September 2026 e2-demo validation used the `aarch64` wheel directly.

The generated community-connector Python source imports
`ebcdic_rust_canary` at runtime; it does not embed platform-specific native
code.

## Output metadata

When `include_file_metadata` is enabled, each row includes:

- `__source_file`;
- `__source_mtime_ns`;
- `__record_index`.

These columns provide stable lineage and can form a downstream uniqueness key.
