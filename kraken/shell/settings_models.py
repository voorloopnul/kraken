"""The Models settings page: every model pi offers, with a checkbox each.

The checked set is pi's own `enabledModels` in `~/.pi/agent/settings.json` —
the setting behind the `--models` flag — rather than a preference of ours, so
the scope chosen here is the scope pi starts a session with and the one its own
`/model` picker cycles through. `kraken.agent.pi_config` holds the contract;
this page is the list in front of it.

The catalogue itself comes from pi (`get_available_models`), because that is
exactly the list the chat footer's picker offers — a page whose checkboxes came
from somewhere else would be a page about a different list.

The tree is a provider, then its models — except behind an aggregator, where it
is a provider, the upstream vendor, then the models. OpenRouter alone lists
hundreds of models under ~60 vendors, all named `vendor/model`; flattened, one
provider's catalogue buries every other, and the vendor a model comes from is
usually how you know whether you want it at all.
"""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QLineEdit, QTreeWidget, QTreeWidgetItem

from kraken.agent import pi_config
from kraken.shell.settings_page import Page, chip, label

# Height of the model tree. Tall enough to browse a provider's catalogue
# without swallowing the page it sits on; the tree scrolls inside it.
_TREE_HEIGHT = 320

# Above this many models the providers open closed: an aggregator like
# OpenRouter lists hundreds, and every other provider would be somewhere below
# them. Under it, the whole catalogue is worth showing at once.
_EXPANDED_MAX = 40

# What an item carries. A leaf's reference is what gets written when it is
# checked; a group's is the plain name it is grouping by, kept apart from the
# text because that gains a count. `_NAME` is what the row is filtered on: the
# full `provider/id` and display name, so a nested model still answers to its
# vendor after the tree has taken that prefix out of its label.
_REF = Qt.ItemDataRole.UserRole
_MODEL = Qt.ItemDataRole.UserRole + 1
_NAME = Qt.ItemDataRole.UserRole + 2


def _vendor(model: dict) -> str:
    """The upstream vendor an aggregator's model id names — `moonshotai` in
    `moonshotai/kimi-k3` — or `""` for a plain id. A leading slash is a path,
    not a vendor: llama.cpp names its models by the file they are loaded from."""
    head, slash, tail = str(model.get("id") or "").partition("/")
    return head if slash and head and tail else ""


def _tally(item: QTreeWidgetItem) -> tuple[int, int]:
    """How many models under a group are checked, of how many there are — at
    any depth, so a provider counts what its vendors hold."""
    if item.childCount() == 0:
        return int(item.checkState(0) == Qt.CheckState.Checked), 1
    checked = total = 0
    for index in range(item.childCount()):
        under, held = _tally(item.child(index))
        checked += under
        total += held
    return checked, total


class ModelsPage(Page):
    """Reads the scope from pi's settings on construction and writes it back on
    every toggle, so the page and the file never drift apart."""

    def __init__(
        self, fetch_models: Callable[[Callable[[list], None]], None] | None = None
    ):
        super().__init__("Settings", "Models")
        self._models: list[dict] = []
        self._noun = ("model", "models")
        # Guards the writes: checking a provider ticks its models, and each of
        # those is another itemChanged we must not treat as a fresh edit.
        self._updating = False

        self.add_section("Models")
        self.add_note(
            "Which models the chat footer's picker offers. Unchecking one hides "
            "it there; it does not remove the provider or its credentials. The "
            f"set is stored as pi's own {pi_config.ENABLED_MODELS} in "
            "settings.json, so pi honours it too — and a change here rewrites "
            "that setting as an explicit list of models."
        )

        self._search = QLineEdit()
        self._search.setPlaceholderText("Filter models…")
        self._search.setClearButtonEnabled(True)
        self._search.textChanged.connect(self._filter)
        self.add_row(
            "Filter",
            "Narrows the list below by model id, name, or provider. It hides "
            "rows only — nothing is checked or unchecked by filtering.",
            self._search,
        )

        self._tree = QTreeWidget()
        self._tree.setObjectName("modelTree")
        self._tree.setHeaderHidden(True)
        self._tree.setFrameShape(QTreeWidget.Shape.NoFrame)
        self._tree.setIndentation(14)
        self._tree.setUniformRowHeights(True)
        self._tree.setMinimumHeight(_TREE_HEIGHT)
        self._tree.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self._tree.itemChanged.connect(self._on_item_changed)
        self.add_widget(self._tree)

        self._all = chip("Check all")
        self._all.clicked.connect(lambda: self._set_all(True))
        self._none = chip("Uncheck all")
        self._none.clicked.connect(lambda: self._set_all(False))
        self.add_actions(self._none, self._all)

        self._status = label("", "description", wrap=True)
        self.add_status(self._status)

        # The answer arrives after the page is built — pi is a process away —
        # so the page opens saying it is waiting rather than showing an empty
        # list that would read as "no models".
        self._pending = fetch_models is not None
        if fetch_models is None:
            self.set_models([])
        else:
            self._report(pi_config.enabled_models())
            fetch_models(self.set_models)

    # ---- The list --------------------------------------------------------

    def set_models(self, models: list[dict]) -> None:
        """Show pi's catalogue, checked against the stored scope. Called when
        the fetch answers, which is usually after the page is on screen."""
        self._pending = False
        self._models = [model for model in models if isinstance(model, dict)]
        # What the rows are counting. The fallback list is patterns, not models,
        # and a count of "models" over it would be a count of the wrong thing.
        self._noun = ("model", "models") if self._models else ("entry", "entries")
        patterns = pi_config.enabled_models()
        self._updating = True
        try:
            self._tree.clear()
            if self._models:
                self._build_models(patterns)
            else:
                self._build_patterns(patterns)
            self._relabel_all()
        finally:
            self._updating = False
        self._filter(self._search.text())
        self._report(patterns)

    def _build_models(self, patterns: list[str]) -> None:
        """A group per provider, its models under it — under their vendor first
        where the provider is an aggregator naming models `vendor/model`."""
        scoped = {
            pi_config.model_ref(model)
            for model in pi_config.in_scope(self._models, patterns)
        }
        by_provider: dict[str, list[dict]] = {}
        for model in self._models:
            by_provider.setdefault(str(model.get("provider") or "unknown"), []).append(
                model
            )
        for provider, models in by_provider.items():
            group = self._add_group(self._tree, provider)
            # One vendor is not a division worth a level of tree: it would put
            # every model of a single-vendor provider one indent further in for
            # a heading that repeats the provider.
            nested = len({_vendor(model) for model in models} - {""}) > 1
            vendors: dict[str, QTreeWidgetItem] = {}
            for model in models:
                vendor = _vendor(model) if nested else ""
                parent = group
                if vendor:
                    if vendor not in vendors:
                        vendors[vendor] = self._add_group(group, vendor)
                    parent = vendors[vendor]
                self._add_model(parent, model, scoped, patterns)
        for index in range(self._tree.topLevelItemCount()):
            self._finish_group(self._tree.topLevelItem(index))

    def _add_group(self, parent, name: str) -> QTreeWidgetItem:
        """A heading row: checkable, but not a scope entry of its own, so it
        keeps its name in its data rather than in the text it shows — the text
        gains a count once the group is full."""
        group = QTreeWidgetItem(parent, [name])
        group.setFlags(
            (group.flags() | Qt.ItemFlag.ItemIsUserCheckable)
            & ~Qt.ItemFlag.ItemIsSelectable
        )
        group.setData(0, _REF, name)
        group.setData(0, _NAME, name)
        return group

    def _add_model(
        self,
        parent: QTreeWidgetItem,
        model: dict,
        scoped: set[str],
        patterns: list[str],
    ) -> None:
        ref = pi_config.model_ref(model)
        item = QTreeWidgetItem(parent, [self._model_text(model, parent)])
        item.setFlags(item.flags() | Qt.ItemFlag.ItemIsUserCheckable)
        item.setData(0, _REF, ref)
        item.setData(0, _MODEL, model)
        item.setData(0, _NAME, f"{ref} {model.get('name') or ''}")
        item.setCheckState(
            0,
            Qt.CheckState.Checked
            if not patterns or ref in scoped
            else Qt.CheckState.Unchecked,
        )

    def _finish_group(self, group: QTreeWidgetItem) -> None:
        """Take a group's state from its rows, deepest first — a vendor's box
        has to be right before the provider's box can be taken from it."""
        for index in range(group.childCount()):
            child = group.child(index)
            if child.childCount():
                self._finish_group(child)
        self._sync_group(group)

    def _relabel(self, group: QTreeWidgetItem) -> None:
        """Name a group by how much of it is checked. The counts are on the row
        because the row is often all you see: a closed provider should say what
        it holds and how much of that is on offer, without being opened."""
        for index in range(group.childCount()):
            child = group.child(index)
            if child.childCount():
                self._relabel(child)
        checked, total = _tally(group)
        noun = self._noun[0] if total == 1 else self._noun[1]
        group.setText(0, f"{group.data(0, _NAME)}  ·  {checked} of {total} {noun}")

    def _relabel_all(self) -> None:
        for index in range(self._tree.topLevelItemCount()):
            self._relabel(self._tree.topLevelItem(index))

    def _build_patterns(self, patterns: list[str]) -> None:
        """The fallback with nothing to list: the stored scope itself, one row
        per pattern. Unchecking still works, which is what makes this worth
        showing rather than an empty page."""
        if not patterns:
            return
        group = self._add_group(self._tree, "settings.json")
        for pattern in patterns:
            item = QTreeWidgetItem(group, [pattern])
            item.setFlags(item.flags() | Qt.ItemFlag.ItemIsUserCheckable)
            item.setData(0, _REF, pattern)
            item.setData(0, _NAME, pattern)
            item.setCheckState(0, Qt.CheckState.Checked)
        self._sync_group(group)

    def _model_text(self, model: dict, parent: QTreeWidgetItem) -> str:
        """A model's row. Under its vendor the id sheds that prefix: the vendor
        is the row above, and repeating it costs the width the id needs."""
        name = str(model.get("name") or "")
        model_id = str(model.get("id") or "")
        vendor = _vendor(model)
        if vendor and parent.data(0, _REF) == vendor:
            model_id = model_id[len(vendor) + 1 :]
        return f"{model_id}  ·  {name}" if name and name != model_id else model_id

    # ---- Editing ---------------------------------------------------------

    def _on_item_changed(self, item: QTreeWidgetItem, column: int) -> None:
        if self._updating:
            return
        self._updating = True
        try:
            if item.childCount():
                # A heading is a switch for what is under it, not a scope entry
                # of its own: pi's list names models.
                self._apply_state(item, item.checkState(0))
            self._sync_ancestors(item)
            self._relabel_all()
        finally:
            self._updating = False
        self._save()

    def _set_all(self, checked: bool) -> None:
        state = Qt.CheckState.Checked if checked else Qt.CheckState.Unchecked
        self._updating = True
        try:
            for index in range(self._tree.topLevelItemCount()):
                group = self._tree.topLevelItem(index)
                group.setCheckState(0, state)
                self._apply_state(group, state)
            self._relabel_all()
        finally:
            self._updating = False
        self._save()

    def _apply_state(self, group: QTreeWidgetItem, state: Qt.CheckState) -> None:
        """Hand a group's state down to every row beneath it."""
        for index in range(group.childCount()):
            child = group.child(index)
            child.setCheckState(0, state)
            self._apply_state(child, state)

    def _sync_ancestors(self, item: QTreeWidgetItem) -> None:
        """Take every heading above `item` from what it now holds. A vendor can
        go partial while its provider goes from checked to partial too, so this
        walks all the way to the top rather than one level."""
        parent = item.parent()
        while parent is not None:
            self._sync_group(parent)
            parent = parent.parent()

    def _sync_group(self, group: QTreeWidgetItem) -> None:
        """A heading's own box follows its rows: on, off, or partial."""
        states = {
            group.child(index).checkState(0) for index in range(group.childCount())
        }
        if states == {Qt.CheckState.Checked}:
            group.setCheckState(0, Qt.CheckState.Checked)
        elif states == {Qt.CheckState.Unchecked}:
            group.setCheckState(0, Qt.CheckState.Unchecked)
        else:
            group.setCheckState(0, Qt.CheckState.PartiallyChecked)

    def checked_refs(self) -> list[str]:
        """The references the checked rows stand for, in the order shown.
        Headings are skipped: only a model is a scope entry."""
        refs: list[str] = []

        def walk(item: QTreeWidgetItem) -> None:
            for index in range(item.childCount()):
                child = item.child(index)
                if child.childCount():
                    walk(child)
                elif child.checkState(0) == Qt.CheckState.Checked:
                    refs.append(str(child.data(0, _REF)))

        for index in range(self._tree.topLevelItemCount()):
            walk(self._tree.topLevelItem(index))
        return refs

    def _orphan_patterns(self, patterns: list[str]) -> list[str]:
        """Stored patterns that no listed model matches — a scope entry for a
        provider whose credentials are gone for the moment, say. They are kept
        on every write: they are not ours to drop, and the page never showed
        them to be unchecked."""
        return [
            pattern
            for pattern in patterns
            if not any(
                pi_config.matches_pattern(model, pattern) for model in self._models
            )
        ]

    def _save(self) -> None:
        checked = self.checked_refs()
        if not checked:
            # An empty list means "every model" to pi, which is the opposite of
            # what an empty page says. Nothing is written until a box goes back
            # on; the scope on disk stays as it was.
            self._status.setText(
                "Nothing checked. Check at least one model — an empty list is "
                "how pi says “no scope at all”, so the saved scope is left as "
                "it was until then."
            )
            return
        patterns = pi_config.enabled_models()
        kept = self._orphan_patterns(patterns) if self._models else []
        # Everything checked and nothing else in the file: drop the setting
        # rather than pin today's catalogue, so a model added later shows up.
        if self._models and len(checked) == len(self._models) and not kept:
            pi_config.save_enabled_models([])
        else:
            pi_config.save_enabled_models(checked + kept)
        self._report(pi_config.enabled_models())

    # ---- State -----------------------------------------------------------

    def _filter(self, query: str) -> None:
        words = query.strip().lower().split()
        # Closed by default once there are more models than anyone wants to
        # scroll past; the tri-state boxes still say how each group stands.
        expanded = len(self._models) <= _EXPANDED_MAX
        for index in range(self._tree.topLevelItemCount()):
            self._filter_item(self._tree.topLevelItem(index), words, expanded)

    def _filter_item(
        self, item: QTreeWidgetItem, words: list[str], expanded: bool
    ) -> int:
        """Hide what does not match and answer with how many models are left
        showing, so a group can hide itself when nothing under it survived and
        open itself when something did."""
        if not item.childCount():
            # Matched on the model's full reference and display name rather than
            # the row's text: a nested row shows its id without the vendor, and
            # searching for the vendor must still find it.
            haystack = str(item.data(0, _NAME) or item.text(0)).lower()
            hidden = not all(word in haystack for word in words)
            item.setHidden(hidden)
            return 0 if hidden else 1
        shown = sum(
            self._filter_item(item.child(index), words, expanded)
            for index in range(item.childCount())
        )
        item.setHidden(bool(words) and not shown)
        item.setExpanded(bool(shown) if words else expanded)
        return shown

    def _report(self, patterns: list[str]) -> None:
        if self._pending:
            self._status.setText("Asking pi which models it offers…")
            return
        if not self._models:
            if patterns:
                self._status.setText(
                    "pi offered no model list. The "
                    f"{len(patterns)} entr{'y' if len(patterns) == 1 else 'ies'} "
                    "above are the scope as settings.json has it, and unchecking "
                    "one still drops it."
                )
            else:
                self._status.setText(
                    "pi offered no models. A provider needs credentials before "
                    "pi lists what it has — Providers is where they go — and "
                    "until one does, there is nothing here to choose between."
                )
            return
        total = len(self._models)
        offered = len(pi_config.in_scope(self._models, patterns))
        if offered == total:
            self._status.setText(f"All {total} models are offered in the picker.")
        else:
            self._status.setText(
                f"{offered} of {total} models offered; the other {total - offered} "
                "are hidden from the picker."
            )
