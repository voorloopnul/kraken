"""The settings window's frame: the navbar and its search, the breadcrumb and
its back arrow, and a theme choice that reaches the rest of the app as it is
picked rather than on close."""

from kraken.chat.typography import MAX_SIZE, MIN_SIZE
from kraken.shell.settings_dialog import SettingsDialog
from kraken.shell.workspace_bar import WorkspaceBar


def _categories(dialog: SettingsDialog) -> list[str]:
    tree = dialog._tree
    return [tree.topLevelItem(i).text(0) for i in range(tree.topLevelItemCount())]


def test_gear_sits_between_theme_and_quit(qapp):
    bar = WorkspaceBar()
    layout = bar.layout()
    order = [
        layout.itemAt(i).widget()
        for i in range(layout.count())
        if layout.itemAt(i).widget() is not None
    ]
    names = {v: k for k, v in bar.buttons.items()}
    tail = [names.get(w) for w in order if w in names]
    assert tail == ["Toggle Theme", "Settings", "Quit"]
    assert not bar.buttons["Settings"].icon().isNull()


def test_categories_switch_pages(qapp):
    dialog = SettingsDialog(theme_name="dark")
    assert _categories(dialog) == ["General", "Theme", "Models", "Providers"]
    assert dialog._pages.count() == 4
    dialog._tree.setCurrentItem(dialog._tree.topLevelItem(2))
    assert dialog._pages.currentIndex() == 2
    assert dialog._breadcrumb.text().startswith("Settings / ")
    assert "Models" in dialog._breadcrumb.text()


def test_search_filters_the_navbar_without_navigating(qapp):
    dialog = SettingsDialog(theme_name="dark")
    dialog._search.setText("mod")
    hidden = [
        dialog._tree.topLevelItem(i).isHidden()
        for i in range(dialog._tree.topLevelItemCount())
    ]
    assert hidden == [True, True, False, True]
    # Filtering is not navigation: the page you were on is still the one shown.
    assert dialog._pages.currentIndex() == 0
    dialog._search.clear()
    assert not any(
        dialog._tree.topLevelItem(i).isHidden()
        for i in range(dialog._tree.topLevelItemCount())
    )


def test_back_returns_to_the_previous_page(qapp):
    dialog = SettingsDialog(theme_name="dark")
    assert not dialog._back.isEnabled()
    dialog._tree.setCurrentItem(dialog._tree.topLevelItem(2))
    assert dialog._back.isEnabled()
    dialog._back.click()
    assert dialog._pages.currentIndex() == 0
    # The navbar follows the page rather than being left on the old row, and
    # one step back from a single hop exhausts the history.
    assert dialog._tree.currentItem() is dialog._tree.topLevelItem(0)
    assert not dialog._back.isEnabled()


def test_theme_choice_is_emitted_live(qapp):
    dialog = SettingsDialog(theme_name="dark")
    picked = []
    dialog.theme_selected.connect(picked.append)
    dialog._theme_picker.setCurrentIndex(dialog._theme_picker.findData("light"))
    # Emitted on the choice, not on close, and the dialog restyled itself with
    # the app rather than staying dark against a light window.
    assert picked == ["light"]
    assert dialog._theme_name == "light"


def test_theme_selection_starts_on_the_current_theme(qapp):
    dialog = SettingsDialog(theme_name="light")
    assert dialog._theme_picker.currentData() == "light"


def test_font_size_starts_where_the_app_is_and_emits_on_change(qapp):
    dialog = SettingsDialog(theme_name="dark", font_size=15)
    assert dialog._font_picker.value() == 15
    picked = []
    dialog.font_size_selected.connect(picked.append)
    dialog._font_picker.setValue(17)
    assert picked == [17]


def test_font_size_is_bounded_by_what_the_chat_supports(qapp):
    dialog = SettingsDialog(theme_name="dark", font_size=MAX_SIZE + 5)
    # A stored value past the range is pulled back rather than offered.
    assert dialog._font_picker.value() == MAX_SIZE
    assert dialog._font_picker.minimum() == MIN_SIZE
    assert dialog._font_picker.maximum() == MAX_SIZE


def test_a_late_model_list_does_not_reach_a_closed_dialog(qapp):
    """The window's fetch answers on a timeout even when pi never does, so it
    can outlive the dialog it was started for — and the dialog is deleted when
    it closes. The reply has to notice."""
    import shiboken6
    from PySide6.QtCore import QEvent

    held = []
    dialog = SettingsDialog(theme_name="dark", fetch_models=held.append)
    dialog.close()
    dialog.deleteLater()
    # deleteLater posts a deferred delete, which only the event loop collects —
    # in the app that is the loop `exec()` returned to. Collected for this
    # dialog alone: passing None would reap every other object's pending
    # deletion too, including ones another test is still holding.
    qapp.sendPostedEvents(dialog, QEvent.Type.DeferredDelete)
    assert not shiboken6.isValid(dialog)
    # Would raise on a destroyed page without the guard.
    held[0]([{"provider": "anthropic", "id": "claude-opus-4-8"}])
