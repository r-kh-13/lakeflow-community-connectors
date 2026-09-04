# End-to-end validation

Validated on 3 September 2026 in the `e2-demo-field-eng` workspace.

- Community connection: `ebcdic_cobol_e2e_20260903`
- Pipeline ID: `75d3a40c-0fa3-4ad0-86b7-c3914a20457e`
- Successful update: `501a685f-8623-4b16-911b-f7d2a5c1e6a1`
- Python 3.10 ABI / production-package update:
  `4ac37b87-87f8-4923-8b28-df7f2e22a110`
- Pipeline URL:
  `https://e2-demo-field-eng.cloud.databricks.com/pipelines/75d3a40c-0fa3-4ad0-86b7-c3914a20457e`
- Destination:
  `users.reda_khouani.ebcdic_customers_e2e_20260903`

The managed serverless pipeline read fixed-length EBCDIC files from a Unity
Catalog Volume through the custom `COMMUNITY` connector and wrote typed Delta
rows:

```text
NAME=ALICE
CUSTOMER_ID=42
AMOUNT=123.45
__record_index=0
```

Validated destination types:

```text
NAME                 string
CUSTOMER_ID          int
AMOUNT               decimal(5,2)
__source_file        string
__source_mtime_ns    bigint
__record_index       bigint
```

The latest run used
`lakeflow_ebcdic_decoder-0.1.0-cp310-abi3-manylinux2014_aarch64.whl`.
It appended exactly one row from `customers-pr-ready.dat`, bringing the table
to two rows. Managed ingestion partial analysis used the explicit manifest
schema; runtime records were decoded with the copybook and Rust extension.

The FE vending-machine workspace used for the earlier classic Cobrix canary
could not run this test because `CONNECTION_COMMUNITY` was not enabled there.
