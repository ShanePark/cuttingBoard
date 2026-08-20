from __future__ import annotations

from typing import Protocol

from cutting_board.models import WorkspaceSnapshot


class ServiceScanner(Protocol):
    def scan(self) -> WorkspaceSnapshot:
        """Return a point-in-time snapshot of locally listening services."""
