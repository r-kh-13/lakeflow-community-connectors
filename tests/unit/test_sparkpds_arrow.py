"""Tests for direct Arrow batch passthrough in the Python Data Source bridge."""

from pyspark.sql import Row
from pyspark.sql.types import StringType, StructField, StructType

from databricks.labs.community_connector.sparkpds.lakeflow_datasource import (
    _parse_or_passthrough_records,
)


def test_record_batch_is_passed_through_without_row_conversion():
    record_batch_type = type("RecordBatch", (), {"__module__": "pyarrow.lib"})
    batch = record_batch_type()
    assert list(_parse_or_passthrough_records([batch], StructType([]))) == [batch]


def test_regular_records_still_use_framework_conversion():
    schema = StructType([StructField("value", StringType(), nullable=False)])
    rows = list(_parse_or_passthrough_records([{"value": "ok"}], schema))
    assert rows == [Row(value="ok")]


def test_empty_iterator_remains_empty():
    assert list(_parse_or_passthrough_records(iter(()), StructType([]))) == []
