# Banking stress validation

Validated on 3 September 2026 in `e2-demo-field-eng` using managed serverless
Lakeflow ingestion and the custom `COMMUNITY` connector.

## Infrastructure

- Pipeline:
  `https://e2-demo-field-eng.cloud.databricks.com/pipelines/6c7b9ca4-4b0b-464a-97af-d467c43b4d23`
- Initial update: `f649d094-c61f-4c0f-903c-f271fd5fe785`
- Idempotence update: `011bd36c-2d25-4a78-bd26-0e08b66650fa`
- Incremental update: `2f21ae68-583f-4574-850e-9fb296e2dd56`
- Native runtime: ABI3 manylinux2014 `aarch64`

## Initial load

Five source tables ran concurrently:

| Source shape | Rows | Validated metric |
|---|---:|---:|
| Fixed `F`, 20 files | 1,000,000 | `SUM(AMOUNT) = 912,000,710,010.00` |
| Fixed `F`, gzip | 50,000 | `SUM(AMOUNT) = 90,599,715,751.00` |
| Variable `V`, RDW + nested ODO | 10,000 | 19,999 legs; `SUM(LEG_AMOUNT) = 502,365,295.89` |
| Variable blocked `VB`, BDW + RDW | 10,000 | debit `2,187,073,150.00`; credit `2,187,650,000.00` |
| Extreme/malformed COMP-3 | 6 | one malformed value mapped to `NULL`; min/max ±9,999,999.99 |

All generated oracle counts and decimal sums matched the Delta tables exactly.
The ledger intentionally contains a `576,850.00` credit surplus, demonstrating
that downstream reconciliation can detect a real financial imbalance without
numeric drift from decoding.

The initial update took 110.4 seconds wall-clock including serverless startup.
The five flows ran for approximately 40 seconds. The one-million-row fixed flow
completed in 36.4 seconds end-to-end (about 27,500 rows/second including Delta
write and flow overhead).

## Idempotence

The second update completed with all row counts unchanged:

- fixed transactions: 1,000,000;
- gzip transactions: 50,000;
- payments: 10,000;
- ledger: 10,000;
- edge values: 6.

The active execution phase took 6.6 seconds. No source file was re-ingested.

## Incremental append

A new 50,000-row fixed file was uploaded after the committed offset.
The next update:

- increased only the fixed table to 1,050,000 rows;
- added exactly 50,000 rows bearing the new `__source_file`;
- increased `SUM(AMOUNT)` by exactly `600,190,750.00`;
- left all other tables unchanged.

The active execution phase took 23.8 seconds.

## Corrupt framing

An isolated negative-test pipeline used a zero RDW header:

- Pipeline:
  `https://e2-demo-field-eng.cloud.databricks.com/pipelines/9fbfb1da-584e-4f8a-976b-cd5b0a5aaad8`
- Update: `27ea1d39-baf4-4653-9fa3-2c74e573c009`
- Expected state: `FAILED`
- Exact worker error:
  `RDW headers should never be zero (0,0,0,0). Found zero size record at 0.`

The framing failure remained isolated and did not affect any valid destination
table.

## Main finding

The fixed, RDW/ODO, VB, gzip, decimal edge, idempotence, incremental, and
fail-fast paths all behaved correctly. Gzip remains the obvious performance
target: it is intentionally handled as a single compressed input partition,
while uncompressed fixed files scale across one Spark partition per file.
