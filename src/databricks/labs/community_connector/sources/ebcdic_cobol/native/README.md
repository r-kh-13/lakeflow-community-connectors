# Lakeflow EBCDIC decoder

Rust/PyO3 decoder used by the EBCDIC COBOL community connector.

The extension uses the CPython stable ABI from Python 3.10 and is built for
both Linux architectures used by Databricks serverless:

```bash
maturin build --release --target x86_64-unknown-linux-gnu --zig --manylinux 2014
maturin build --release --target aarch64-unknown-linux-gnu --zig --manylinux 2014
```

Expected artifacts:

```text
lakeflow_ebcdic_decoder-0.1.0-cp310-abi3-manylinux2014_x86_64.whl
lakeflow_ebcdic_decoder-0.1.0-cp310-abi3-manylinux2014_aarch64.whl
```

## Distribution prerequisite

Before the connector can use the standard `community-connector
create_pipeline` build path, publish both wheels under the same
`lakeflow-ebcdic-decoder` version to the package index available to pipeline
environment resolution. Then:

1. add `lakeflow-ebcdic-decoder>=0.1.0,<0.2.0` to the connector
   `pyproject.toml`;
2. regenerate `requirements/sources.txt` on the required Linux build host;
3. remove the manual native-wheel path from deployment specs.

Until that publication is complete, deployment must supply a prebuilt native
wheel matching the serverless architecture in `environment.dependencies`.
