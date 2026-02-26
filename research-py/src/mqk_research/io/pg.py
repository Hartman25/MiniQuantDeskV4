from __future__ import annotations

from dataclasses import dataclass

from sqlalchemy import create_engine, text
from sqlalchemy.engine import Engine


@dataclass(frozen=True)
class PgConfig:
    # Explicit, no hidden config. Provide a URL.
    url: str


def make_engine(cfg: PgConfig) -> Engine:
    # SQLAlchemy chosen for:
    # - deterministic synchronous execution
    # - clear explicit SQL + explicit ordering
    # - simple DSN handling via psycopg driver
    return create_engine(cfg.url, future=True, pool_pre_ping=True)


def table_exists(engine: Engine, table_name: str, schema: str = "public") -> bool:
    q = text(
        """
        select 1
        from information_schema.tables
        where table_schema = :schema and table_name = :table
        limit 1
        """
    )
    with engine.connect() as cxn:
        row = cxn.execute(q, {"schema": schema, "table": table_name}).fetchone()
        return row is not None