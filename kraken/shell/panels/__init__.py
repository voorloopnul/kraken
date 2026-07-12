"""The workspace content panels, one module per panel."""

from kraken.shell.panels.base import Card, Panel
from kraken.shell.panels.browser import BrowserPanel
from kraken.shell.panels.center import CenterPanel
from kraken.shell.panels.git import GitPanel
from kraken.shell.panels.left import LeftPanel
from kraken.shell.panels.right import RightPanel

__all__ = [
    "BrowserPanel",
    "Card",
    "CenterPanel",
    "GitPanel",
    "LeftPanel",
    "Panel",
    "RightPanel",
]
