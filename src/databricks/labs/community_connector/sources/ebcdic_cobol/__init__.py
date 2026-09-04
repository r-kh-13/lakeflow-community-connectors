"""EBCDIC/COBOL Lakeflow community connector."""

from databricks.labs.community_connector.sources.ebcdic_cobol.ebcdic_cobol import (
    EbcdicCobolLakeflowConnect,
)
from databricks.labs.community_connector.sparkpds import LakeflowSource


class EbcdicCobolDataSource(LakeflowSource):
    """Spark Python Data Source wrapper for the EBCDIC connector."""

    _lakeflow_connect_cls = EbcdicCobolLakeflowConnect


__all__ = ["EbcdicCobolDataSource", "EbcdicCobolLakeflowConnect"]
