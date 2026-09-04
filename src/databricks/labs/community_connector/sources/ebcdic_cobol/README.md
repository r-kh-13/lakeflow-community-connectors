# EBCDIC COBOL Lakeflow Community Connector

The EBCDIC COBOL connector incrementally loads immutable mainframe files from
Unity Catalog Volumes into Delta tables. A COBOL copybook defines each table's
record layout and Spark schema. Decoding runs in a Rust extension on Spark
workers; Lakeflow manages file discovery, parallelism, checkpoints, retries,
and destination tables.

This is a Lakeflow **community connector**. It is not the Cobrix JVM DataSource
and does not require a classic cluster, SparkContext, RDDs, or a custom JAR.

## Prerequisites

- A Databricks workspace with Community Connections enabled.
- A Unity Catalog Volume containing source files, copybooks, and the connector
  manifest.
- `READ VOLUME` on every referenced Volume path.
- `USE CATALOG`, `USE SCHEMA`, `CREATE TABLE`, and `MODIFY` on the destination.
- Python 3.10 or later.
- The connector wheel and a `lakeflow-ebcdic-decoder` wheel compatible with the
  serverless architecture.

All runtime paths are restricted to `/Volumes/...`.

## Source layout

One manifest can expose multiple logical tables. A typical Volume layout is:

```text
/Volumes/<catalog>/<schema>/<volume>/ebcdic/
├── config/manifest.json
├── copybooks/
│   ├── customers.cpy
│   ├── transactions.cpy
│   └── includes/
└── landing/
    ├── customers/*.dat
    └── transactions/*.dat.gz
```

Files are immutable input units. Publish a new file instead of overwriting an
existing path, and do not backdate its modification time.

## Quick start

### 1. Create a copybook

```cobol
       01 CUSTOMER-RECORD.
          05 CUSTOMER-ID PIC 9(10) COMP.
          05 NAME        PIC X(30).
          05 BALANCE     PIC S9(13)V9(2) COMP-3.
          05 STATUS      PIC X(2).
```

### 2. Create the manifest

```json
{
  "tables": {
    "customers": {
      "data_path": "/Volumes/main/finance/ebcdic/landing/customers",
      "copybook_path": "/Volumes/main/finance/ebcdic/copybooks/customers.cpy",
      "copybook_library_path": "/Volumes/main/finance/ebcdic/copybooks/includes",
      "schema": [
        {"name": "CUSTOMER_ID", "type": "long"},
        {"name": "NAME", "type": "string"},
        {"name": "BALANCE", "type": "decimal(15,2)"},
        {"name": "STATUS", "type": "string"}
      ],
      "file_glob": "*.dat",
      "recursive": false,
      "record_format": "F",
      "encoding": "EBCDIC",
      "batch_rows": 8192,
      "max_files_per_batch": 1000,
      "include_file_metadata": true
    }
  }
}
```

For managed ingestion, declare `schema` explicitly because Lakeflow performs
partial analysis before executor-side native decoding. For direct Spark Data
Source reads, the connector can infer it from the copybook.

### 3. Create the Community Connection

From a checkout of this repository:

```bash
community-connector create_connection \
  ebcdic_cobol bank_ebcdic_connection \
  --options '{
    "config_path": "/Volumes/main/finance/ebcdic/config/manifest.json"
  }' \
  --spec src/databricks/labs/community_connector/sources/ebcdic_cobol/connector_spec.yaml
```

An inline manifest can be supplied with `config_json`, but `config_path` is
recommended for production.

### 4. Define and create the pipeline

```yaml
name: Bank EBCDIC ingestion
catalog: main
schema: bronze
serverless: true
channel: PREVIEW
ingestion_definition:
  connection_name: bank_ebcdic_connection
  objects:
    - table:
        source_schema: default
        source_table: customers
        destination_catalog: main
        destination_schema: bronze
        destination_table: customers_raw
        connector_options:
          community_connector_options:
            options:
              max_files_per_batch: "500"
```

```bash
community-connector create_pipeline \
  ebcdic_cobol bank-ebcdic-pipeline \
  --pipeline-spec pipeline.yaml
```

Source-specific overrides belong under
`connector_options.community_connector_options.options`, not under
`table_configuration`.

## Manifest reference

Each entry under `tables` supports:

| Option | Required | Default | Description |
|---|---:|---|---|
| `data_path` | Yes | — | Absolute `/Volumes/...` directory containing one table's files. |
| `copybook_path` | Yes | — | Absolute path to the table's root copybook. |
| `copybook_library_path` | No | — | Directory containing `.cpy`, `.cob`, or `.copybook` members referenced by `COPY`. |
| `schema` | Managed ingestion | inferred | List of `{name, type, nullable}` fields. Nested Spark DDL types are accepted. |
| `file_glob` | No | `*` | Filename filter, such as `*.dat`, `*.ebc`, or `*.gz`. |
| `recursive` | No | `false` | Discover files below subdirectories. |
| `record_format` | No | `F` | `F`, `V`, or `VB`. |
| `encoding` | No | `EBCDIC` | Text/display encoding. See Decoder options. |
| `batch_rows` | No | `8192` | Maximum decoded rows retained per native output batch. |
| `max_files_per_batch` | No | `1000` | Admission control for one Lakeflow micro-batch. |
| `variable_size_occurs` | No | `false` | Derive concatenated `F` record sizes from ODO counters. |
| `include_file_metadata` | No | `true` | Add source path, mtime, and record index columns. |
| `null_on_error` | No | `false` | Return `NULL` for malformed field values instead of failing. |

The options listed in `connector_spec.yaml` can also be overridden per pipeline
table through `community_connector_options`.

## Record formats

| Value | Physical layout | Behavior |
|---|---|---|
| `F` | Concatenated fixed-size records | Record size is the copybook's maximum size. File size must be an exact multiple unless `null_on_error=true`. |
| `F` + `variable_size_occurs=true` | Concatenated variable-size records | ODO counters determine each record's consumed bytes and following-field offsets. |
| `V` | Four-byte RDW followed by payload | First two RDW bytes are an unsigned big-endian **payload** length; reserved bytes are ignored. |
| `VB` | BDW block containing RDW records | BDW length includes its four-byte header. Inner RDW lengths are payload lengths. |

Malformed or truncated RDW/BDW framing is always fatal because the next record
boundary cannot be recovered safely.

Gzip is selected by a `.gz` filename. Decompressed rows remain batch-bounded,
but a single compressed file is one Spark partition. Split very large archives
into multiple files for parallelism.

## Supported copybook clauses

- levels `01` through `49`; the single root `01` is collapsed;
- nested groups mapped to Spark structs;
- `PIC` and `PICTURE`;
- `REDEFINES` for primitive fields and groups;
- fixed `OCCURS`;
- `OCCURS min TO max DEPENDING ON field`, including nested ODO;
- `SIGN LEADING SEPARATE` and `SIGN TRAILING SEPARATE`;
- `FILLER`, omitted from output while retaining its bytes;
- levels `66` and `88`, ignored because they do not occupy storage;
- recursive `COPY member` and `COPY ... REPLACING ==old== BY ==new==`.

Copybook field names are normalized from hyphens to underscores.

## COBOL-to-Spark mappings

### PIC and structural mappings

| COBOL definition | Spark type | Notes |
|---|---|---|
| `PIC X(n)` / `PIC A(n)` | `STRING` | Uses the configured text encoding and trimming policy. |
| `PIC X(n) COMP` | `BINARY` | Returns unmodified bytes. |
| `PIC 9(p)` / `S9(p)`, `p <= 9` | `INT` | DISPLAY or supported compact numeric usage. |
| `PIC 9(p)` / `S9(p)`, `10 <= p <= 18` | `BIGINT` | Values retain integer precision. |
| Integral precision above 18 | `DECIMAL(p,0)` | Native binary decoding supports up to 38 digits. |
| `PIC 9(i)V9(s)` / signed equivalent | `DECIMAL(i+s,s)` | Python `Decimal`; no floating-point conversion. |
| `P` scaling positions | `DECIMAL` or integral | Effective precision and scale include the `P` positions. |
| `COMP-1` | `FLOAT` | IBM hexadecimal or IEEE-754, selected per table. |
| `COMP-2` | `DOUBLE` | IBM hexadecimal or IEEE-754, selected per table. |
| Primitive `OCCURS` | `ARRAY<element>` | Fixed or ODO runtime length. |
| Group | `STRUCT<...>` | Preserves copybook nesting. |
| Group `OCCURS` | `ARRAY<STRUCT<...>>` | Supports nested ODO arrays. |
| `FILLER` | omitted | Still contributes to offsets and record size. |

Set `strict_integral_precision=true` to map integral PICs to
`DECIMAL(p,0)`. Set `display_pic_as_string=true` to retain integral DISPLAY
fields as strings, including leading zeroes.

### USAGE mappings

| COBOL usage | Encoding | Byte order / sign |
|---|---|---|
| DISPLAY | EBCDIC or ASCII digits | Leading/trailing signs, separate signs, decimal point/comma, and zoned overpunch. |
| `COMP`, `BINARY`, `COMP-4`, `COMP-5` | Two's-complement binary | Big-endian; signed only when PIC starts with `S`. |
| `COMP-3`, `PACKED-DECIMAL` | Packed BCD | Final sign nibble `C`/`F` positive, `D` negative. |
| `COMP-3U` | Unsigned packed BCD | No mandatory sign nibble. |
| `COMP-9` | Binary | Little-endian Cobrix-compatible extension. |
| `COMP-1` | 32-bit floating point | Format selected by `floating_point_format`. |
| `COMP-2` | 64-bit floating point | Format selected by `floating_point_format`. |

Binary storage follows the IBM digit ranges: 2 bytes through four digits,
4 bytes through nine digits, and 8 bytes through eighteen digits. `COMP-9`
also supports one-byte values through two digits.

## Decoder options

| Option | Values | Default | Effect |
|---|---|---|---|
| `encoding` | `EBCDIC`, `ASCII`, `UTF16`, `HEX`, `RAW` | `EBCDIC` | Applies to alphanumeric and DISPLAY fields; compact numerics ignore text encoding. |
| `string_trimming_policy` | `none`, `left`, `right`, `both`, `keep_all` | `right` | Controls spaces/control-byte retention for string fields. |
| `utf16_big_endian` | boolean | `true` | Selects UTF-16BE or UTF-16LE. |
| `floating_point_format` | `IBM`, `IBM_LE`, `IEEE754`, `IEEE754_LE` | `IBM` | Applies to all `COMP-1` and `COMP-2` fields in the table. |
| `strict_sign_overpunch` | boolean | `false` | Rejects relaxed/multiple overpunch signs. |
| `improved_null_detection` | boolean | `false` | Treats all-zero strings and all-zero/blank DISPLAY numerics as `NULL`. |
| `strict_integral_precision` | boolean | `false` | Returns integral numerics as exact Spark decimals. |
| `display_pic_as_string` | boolean | `false` | Retains integral DISPLAY values as strings. |
| `null_on_error` | boolean | `false` | Converts malformed/truncated fields to `NULL`; does not suppress framing errors. |

`EBCDIC` currently uses the Cobrix common/invariant conversion table. It is not
a configurable full national code page such as IBM037, IBM500, or IBM1140.
Bytes outside the invariant set may map to spaces.

`HEX` returns uppercase hexadecimal strings. `RAW` returns Spark binary values.

## Incremental ingestion

The connector is append-only. Files are sorted by `(mtime_ns, absolute_path)`.
The start offset is exclusive and the end offset is inclusive.

Consequences:

- rerunning without new files produces no duplicate rows;
- a newly published file advances the checkpoint;
- a new file with an older ordering key than the committed offset is skipped;
- deleting or modifying an already committed file does not retract prior rows.

`max_files_per_batch` limits checkpoint advancement and provides admission
control. File discovery is deterministic across retries.

## Output metadata

With `include_file_metadata=true`, every row includes:

| Column | Spark type | Meaning |
|---|---|---|
| `__source_file` | `STRING` | Absolute Volume path of the source file. |
| `__source_mtime_ns` | `BIGINT` | File modification timestamp used by the cursor. |
| `__record_index` | `BIGINT` | Zero-based record index within the source file. |

Together, `(__source_file, __record_index)` is a stable source-row identifier.

## Error handling

- Copybook syntax, unsupported mappings, missing COPY members, and invalid ODO
  counts fail compilation.
- With `null_on_error=false`, malformed field content fails the partition.
- With `null_on_error=true`, malformed field content becomes `NULL`.
- `improved_null_detection=true` distinguishes blank/null DISPLAY values.
- Framing errors always fail with the byte offset and offending RDW/BDW.

Use an isolated landing path or pipeline while validating unfamiliar files
before mixing them into a production table.

## Performance guidance

- Use multiple immutable files to expose parallel Spark partitions.
- Keep `batch_rows` between 4,096 and 16,384 unless row width is extreme.
- Use `max_files_per_batch` to bound one update.
- Prefer uncompressed files for maximum parallel throughput.
- Split large gzip archives; one archive cannot be divided across executors.
- Copybooks and COPY libraries are cached by path and modification time.
- Uncompressed files are memory-mapped; decoded output remains batch-bounded.

## Current limitations

- no custom EBCDIC code-page selection;
- no edited PIC symbols such as `Z`, `$`, `*`, `CR`, or `DB`;
- no COBOL `RENAMES`;
- no multisegment discriminator/rule system;
- no little-endian or header-adjusted RDW/BDW options;
- no recovery after corrupt record framing;
- files must be immutable and monotonically ordered by mtime/path.

See `THIRD_PARTY_NOTICES.md` for Cobrix and `ibm2ieee` attribution.
