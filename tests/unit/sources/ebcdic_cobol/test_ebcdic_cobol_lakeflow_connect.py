"""Unit tests for the EBCDIC/COBOL Volume connector."""

from __future__ import annotations

import gzip
import json
import os
from decimal import Decimal

from databricks.labs.community_connector.sources.ebcdic_cobol.ebcdic_cobol import (
    EbcdicCobolLakeflowConnect,
)

_COPYBOOK = """
       01 RECORD.
          05 NAME PIC X(8).
          05 CUSTOMER_ID PIC 9(4).
          05 AMOUNT PIC S9(3)V9(2) COMP-3.
"""
_RECORD = bytes.fromhex("c1d3c9c3c5404040f0f0f4f212345c")


def _connector(tmp_path, *, max_files_per_batch=1000, declared_schema=False):
    data_path = tmp_path / "data"
    data_path.mkdir()
    copybook_path = tmp_path / "customers.cpy"
    copybook_path.write_text(_COPYBOOK)
    table_config = {
        "data_path": str(data_path),
        "copybook_path": str(copybook_path),
        "file_glob": "*.dat*",
        "record_format": "F",
        "max_files_per_batch": max_files_per_batch,
    }
    if declared_schema:
        table_config["schema"] = [
            {"name": "NAME", "type": "string"},
            {"name": "CUSTOMER_ID", "type": "integer"},
            {"name": "AMOUNT", "type": "decimal(5,2)"},
        ]
    manifest = {
        "tables": {
            "customers": table_config,
        }
    }
    return (
        EbcdicCobolLakeflowConnect({"config_json": json.dumps(manifest)}),
        data_path,
    )


def test_schema_and_native_partition_read(tmp_path):
    connector, data_path = _connector(tmp_path)
    source = data_path / "customers-001.dat"
    source.write_bytes(_RECORD * 2)

    schema = connector.get_table_schema("customers", {})
    assert [field.name for field in schema] == [
        "NAME",
        "CUSTOMER_ID",
        "AMOUNT",
        "__source_file",
        "__source_mtime_ns",
        "__record_index",
    ]
    partition = connector.get_partitions("customers", {})[0]
    rows = list(connector.read_partition("customers", partition, {}))
    assert rows[0]["NAME"] == "ALICE"
    assert rows[0]["CUSTOMER_ID"] == 42
    assert rows[0]["AMOUNT"] == Decimal("123.45")
    assert rows[1]["__record_index"] == 1
    assert rows[0]["__source_file"] == str(source)


def test_declared_schema_does_not_load_native_decoder(monkeypatch, tmp_path):
    connector, _ = _connector(tmp_path, declared_schema=True)
    monkeypatch.setattr(
        connector,
        "_compiled_decoder",
        lambda _: (_ for _ in ()).throw(AssertionError("native decoder loaded")),
    )
    schema = connector.get_table_schema("customers", {})
    assert schema.simpleString().startswith(
        "struct<NAME:string,CUSTOMER_ID:int,AMOUNT:decimal(5,2)"
    )


def test_incremental_offsets_cap_files_per_batch(tmp_path):
    connector, data_path = _connector(tmp_path, max_files_per_batch=1)
    first = data_path / "001.dat"
    second = data_path / "002.dat"
    first.write_bytes(_RECORD)
    second.write_bytes(_RECORD)
    os.utime(first, ns=(1_000_000_000, 1_000_000_000))
    os.utime(second, ns=(2_000_000_000, 2_000_000_000))

    end_one = connector.latest_offset("customers", {}, {})
    assert end_one == {"mtime_ns": 1_000_000_000, "path": str(first)}
    assert [item["path"] for item in connector.get_partitions("customers", {}, {}, end_one)] == [
        str(first)
    ]
    end_two = connector.latest_offset("customers", {}, end_one)
    assert end_two == {"mtime_ns": 2_000_000_000, "path": str(second)}


def test_gzip_partition(tmp_path):
    connector, data_path = _connector(tmp_path)
    source = data_path / "customers.dat.gz"
    with gzip.open(source, "wb") as handle:
        handle.write(_RECORD)
    partition = connector.get_partitions("customers", {})[0]
    rows = list(connector.read_partition("customers", partition, {}))
    assert rows[0]["AMOUNT"] == Decimal("123.45")
