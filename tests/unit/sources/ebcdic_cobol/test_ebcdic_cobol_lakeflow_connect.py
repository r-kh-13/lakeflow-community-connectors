"""Unit tests for the EBCDIC/COBOL Volume connector."""

from __future__ import annotations

import gzip
import json
import os
import tempfile
from decimal import Decimal
from pathlib import Path

import pytest

from databricks.labs.community_connector.sources.ebcdic_cobol import (
    ebcdic_cobol as connector_module,
)
from databricks.labs.community_connector.sources.ebcdic_cobol.ebcdic_cobol import (
    EbcdicCobolLakeflowConnect,
)
from tests.unit.sources.test_partition_suite import SupportsPartitionedStreamTests
from tests.unit.sources.test_suite import LakeflowConnectTests

_COPYBOOK = """
       01 RECORD.
          05 NAME PIC X(8).
          05 CUSTOMER_ID PIC 9(4).
          05 AMOUNT PIC S9(3)V9(2) COMP-3.
"""
_RECORD = bytes.fromhex("c1d3c9c3c5404040f0f0f4f212345c")


class _FakeCompiledDecoder:
    """CI stand-in for the separately distributed native wheel."""

    @staticmethod
    def schema():
        return [
            ("NAME", "string", 0, 8, 1),
            ("CUSTOMER_ID", "integer", 8, 4, 1),
            ("AMOUNT", "decimal(5,2)", 12, 3, 1),
        ]

    @staticmethod
    def _batches(data: bytes, batch_size: int):
        rows = [
            {"NAME": "ALICE", "CUSTOMER_ID": 42, "AMOUNT": Decimal("123.45")}
            for _ in range(len(data) // len(_RECORD))
        ]
        for offset in range(0, len(rows), batch_size):
            yield rows[offset : offset + batch_size]

    def iter_batches(self, data: bytes, *, batch_size: int, **_):
        return self._batches(data, batch_size)

    def iter_file_batches(self, path: str, *, batch_size: int, **_):
        return self._batches(Path(path).read_bytes(), batch_size)


@pytest.fixture(autouse=True)
def _fake_native_decoder(monkeypatch):
    monkeypatch.setenv("LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS", "1")
    monkeypatch.setattr(
        connector_module,
        "_compile_decoder",
        lambda *_: _FakeCompiledDecoder(),
    )


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
        "arrow_enabled": False,
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


def test_schema_and_partition_read(tmp_path):
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


def test_arrow_partition_returns_record_batch(tmp_path):
    pa = pytest.importorskip("pyarrow")
    connector, data_path = _connector(tmp_path)
    (data_path / "customers.dat").write_bytes(_RECORD * 2)
    partition = connector.get_partitions("customers", {})[0]
    batches = list(
        connector.read_partition(
            "customers",
            partition,
            {"arrow_enabled": "true"},
        )
    )
    assert len(batches) == 1
    assert isinstance(batches[0], pa.RecordBatch)
    assert batches[0].num_rows == 2
    assert batches[0].schema.field("AMOUNT").type == pa.decimal128(5, 2)
    name_index = batches[0].schema.get_field_index("NAME")
    assert batches[0].column(name_index).to_pylist() == ["ALICE", "ALICE"]


def test_non_volume_paths_require_explicit_local_test_opt_in(monkeypatch, tmp_path):
    monkeypatch.delenv("LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS")
    data_path = tmp_path / "data"
    data_path.mkdir()
    copybook = tmp_path / "customers.cpy"
    copybook.write_text(_COPYBOOK)
    manifest = {
        "tables": {
            "customers": {
                "data_path": str(data_path),
                "copybook_path": str(copybook),
            }
        }
    }
    with pytest.raises(ValueError, match="must be under /Volumes"):
        EbcdicCobolLakeflowConnect({"config_json": json.dumps(manifest)})


class TestEbcdicCobolConnector(
    LakeflowConnectTests,
    SupportsPartitionedStreamTests,
):
    connector_class = EbcdicCobolLakeflowConnect
    simulator_source = "ebcdic_cobol"
    replay_config = {"config_json": "{}"}
    sample_records = 10
    _volume: tempfile.TemporaryDirectory | None = None
    _previous_local_path_setting: str | None = None

    @classmethod
    def setup_class(cls):
        cls._previous_local_path_setting = os.environ.get("LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS")
        os.environ["LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS"] = "1"
        cls._volume = tempfile.TemporaryDirectory(prefix="ebcdic-connector-ci-")
        root = Path(cls._volume.name)
        data = root / "data"
        data.mkdir()
        (data / "customers.dat").write_bytes(_RECORD * 2)
        copybook = root / "customers.cpy"
        copybook.write_text(_COPYBOOK)
        cls.replay_config = {
            "config_json": json.dumps(
                {
                    "tables": {
                        "customers": {
                            "data_path": str(data),
                            "copybook_path": str(copybook),
                            "schema": [
                                {"name": "NAME", "type": "string"},
                                {"name": "CUSTOMER_ID", "type": "integer"},
                                {"name": "AMOUNT", "type": "decimal(5,2)"},
                            ],
                            "file_glob": "*.dat",
                            "record_format": "F",
                            "arrow_enabled": False,
                        }
                    }
                }
            )
        }
        cls.config = None
        try:
            super().setup_class()
        except Exception:
            cls._cleanup()
            raise

    @classmethod
    def teardown_class(cls):
        try:
            super().teardown_class()
        finally:
            cls._cleanup()

    @classmethod
    def _cleanup(cls):
        if cls._volume is not None:
            cls._volume.cleanup()
            cls._volume = None
        if cls._previous_local_path_setting is None:
            os.environ.pop("LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS", None)
        else:
            os.environ["LAKEFLOW_EBCDIC_ALLOW_LOCAL_PATHS"] = cls._previous_local_path_setting
