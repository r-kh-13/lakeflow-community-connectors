# EBCDIC COBOL source contract

The source is a set of immutable files exposed through Unity Catalog Volume
FUSE paths. No external HTTP API or credentials are used.

## Discovery

`config_path` or `config_json` provides a manifest with one configuration per
logical source table. `list_tables()` returns the sorted manifest keys.

For each table, files are discovered below `data_path`, filtered by `file_glob`
and optionally traversed recursively.

## Incremental contract

Files are ordered by `(mtime_ns, absolute_path)`. A streaming offset contains:

```json
{"mtime_ns": 1725393600000000000, "path": "/Volumes/.../customers-001.dat"}
```

The start offset is exclusive and the end offset is inclusive.
`max_files_per_batch` limits how far `latest_offset()` advances in one
micro-batch. Each selected file becomes one Spark partition.

The contract assumes files are immutable and that newly published files do not
have an ordering key older than the committed offset.

## Record contract

Each file uses one copybook and one framing mode:

- `F`: fixed record length from the compiled copybook;
- `F` with `variable_size_occurs=true`: concatenated records whose lengths are
  derived from `OCCURS DEPENDING ON` counters;
- `V`: records prefixed by four-byte RDWs;
- `VB`: blocks prefixed by BDWs containing RDW-framed records.

Malformed values can become `null` using `null_on_error`. Framing errors remain
fatal because continuing would lose record boundaries.

With `arrow_enabled=true` (default), partition readers emit PyArrow
`RecordBatch` objects directly to Spark. Set it to `false` to use the
row-compatible fallback on runtimes without direct Arrow batch support.
