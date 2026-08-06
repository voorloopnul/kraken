"""The Models page: pi's catalogue with a checkbox each, stored as pi's own
`enabledModels`.

What is pinned here is the round trip — what the page shows against what is in
settings.json, and what a toggle writes back — plus the two states that are
easy to get wrong: everything checked (no scope at all, so a model added later
still appears) and nothing checked (which pi would read as no scope, the
opposite of what the page is saying).

Every test redirects PI_CODING_AGENT_DIR, so nothing here can reach the real
~/.pi.
"""

import json

import pytest
from PySide6.QtCore import Qt

from kraken.agent import pi_config
from kraken.shell.settings_models import _REF, ModelsPage

MODELS = [
    {"provider": "anthropic", "id": "claude-opus-4-8", "name": "Claude Opus"},
    {"provider": "anthropic", "id": "claude-sonnet-4-8", "name": "Claude Sonnet"},
    {"provider": "openrouter", "id": "moonshotai/kimi-k3", "name": "Kimi K3"},
]


@pytest.fixture
def agent_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("PI_CODING_AGENT_DIR", str(tmp_path))
    return tmp_path


@pytest.fixture
def page(agent_dir, qapp):
    page = ModelsPage(lambda callback: callback(MODELS))
    yield page
    page.deleteLater()


def scope() -> list[str]:
    """The scope as it stands on disk, which is the only place it lives."""
    return pi_config.enabled_models()


def groups(page) -> list:
    tree = page._tree
    return [tree.topLevelItem(i) for i in range(tree.topLevelItemCount())]


def rows(page) -> dict[str, dict[str, Qt.CheckState]]:
    """The tree as `{provider: {row text: state}}`. A provider row shows a
    count beside its name, so it is keyed by the id it carries in its data."""
    return {
        group.data(0, _REF): {
            group.child(j).text(0): group.child(j).checkState(0)
            for j in range(group.childCount())
        }
        for group in groups(page)
    }


def group(page, provider: str):
    for candidate in groups(page):
        if candidate.data(0, _REF) == provider:
            return candidate
    raise AssertionError(f"no provider row for {provider}")


def item(page, provider: str, index: int):
    return group(page, provider).child(index)


def test_every_model_pi_offers_is_listed_and_checked(page):
    listed = rows(page)
    assert set(listed) == {"anthropic", "openrouter"}
    assert len(listed["anthropic"]) == 2
    assert all(
        state == Qt.CheckState.Checked
        for models in listed.values()
        for state in models.values()
    )
    # Nothing is written just for opening the page: with no scope stored, every
    # model is on offer already.
    assert not pi_config.settings_path().exists()
    assert "All 3 models" in page._status.text()


def test_unchecking_a_model_hides_it_and_stores_the_rest(page):
    item(page, "anthropic", 1).setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["anthropic/claude-opus-4-8", "openrouter/moonshotai/kimi-k3"]
    # And the picker's list is the scope, so the unchecked model is gone from it.
    offered = [model["id"] for model in pi_config.in_scope(MODELS)]
    assert offered == ["claude-opus-4-8", "moonshotai/kimi-k3"]
    assert "2 of 3" in page._status.text()


def test_checking_everything_again_drops_the_setting(page):
    item(page, "openrouter", 0).setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["anthropic/claude-opus-4-8", "anthropic/claude-sonnet-4-8"]
    item(page, "openrouter", 0).setCheckState(0, Qt.CheckState.Checked)
    # Not today's catalogue pinned in a list: no scope at all, so a model added
    # to a provider later is offered without a visit here.
    assert scope() == []
    assert pi_config.ENABLED_MODELS not in json.loads(
        pi_config.settings_path().read_text()
    )


def test_a_provider_row_switches_its_own_models(page):
    anthropic = group(page, "anthropic")
    anthropic.setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["openrouter/moonshotai/kimi-k3"]
    # Unchecking one model leaves the provider partially checked rather than
    # claiming the whole provider is off.
    item(page, "anthropic", 0).setCheckState(0, Qt.CheckState.Checked)
    assert anthropic.checkState(0) == Qt.CheckState.PartiallyChecked


def test_nothing_checked_is_refused_rather_than_saved(page):
    item(page, "anthropic", 0).setCheckState(0, Qt.CheckState.Unchecked)
    saved = scope()
    page._none.click()
    # An empty list is how pi says "no scope"; writing it would offer every
    # model, which is the opposite of an empty page. The last scope stands.
    assert scope() == saved
    assert "at least one model" in page._status.text()
    page._all.click()
    assert scope() == []


def test_a_stored_scope_is_shown_as_the_checked_rows(agent_dir, qapp):
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["anthropic/*"]})
    )
    page = ModelsPage(lambda callback: callback(MODELS))
    listed = rows(page)
    assert set(listed["anthropic"].values()) == {Qt.CheckState.Checked}
    assert set(listed["openrouter"].values()) == {Qt.CheckState.Unchecked}
    page.deleteLater()


def test_a_pattern_matching_nothing_listed_survives_a_change(agent_dir, qapp):
    # A scope entry for a provider that is not in this catalogue — its key is
    # gone for the moment, say. The page never showed it, so it is not the
    # page's to drop.
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["anthropic/*", "openai/gpt-5.5"]})
    )
    page = ModelsPage(lambda callback: callback(MODELS))
    item(page, "anthropic", 0).setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["anthropic/claude-sonnet-4-8", "openai/gpt-5.5"]
    page.deleteLater()


def test_filtering_hides_rows_without_changing_them(page):
    page._search.setText("sonnet")
    assert item(page, "anthropic", 0).isHidden()
    assert not item(page, "anthropic", 1).isHidden()
    assert item(page, "openrouter", 0).isHidden()
    # Hidden is not unchecked, and nothing was written.
    assert item(page, "anthropic", 0).checkState(0) == Qt.CheckState.Checked
    assert scope() == []
    page._search.clear()
    assert not item(page, "anthropic", 0).isHidden()


def test_with_no_session_the_stored_scope_is_still_editable(agent_dir, qapp):
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["anthropic/*", "openai/gpt-5.5"]})
    )
    # pi answered with nothing — not installed, or no provider configured yet.
    # The page shows the stored scope itself rather than an empty list, so the
    # one thing it can still edit is in front of the user.
    page = ModelsPage(lambda callback: callback([]))
    assert list(rows(page)) == ["settings.json"]
    assert list(rows(page)["settings.json"]) == ["anthropic/*", "openai/gpt-5.5"]
    assert "no model list" in page._status.text()
    item(page, "settings.json", 1).setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["anthropic/*"]
    page.deleteLater()


def test_the_page_says_it_is_waiting_before_the_answer(agent_dir, qapp):
    # pi is a process away, so the list arrives after the page is on screen.
    # An empty tree with no word about it would read as "no models".
    held = []
    page = ModelsPage(held.append)
    assert page._tree.topLevelItemCount() == 0
    assert "Asking pi" in page._status.text()
    held[0](MODELS)
    assert page._tree.topLevelItemCount() == 2
    assert "All 3 models" in page._status.text()
    page.deleteLater()


def test_without_a_fetcher_nothing_is_claimed_about_the_models(agent_dir, qapp):
    page = ModelsPage()
    assert page._tree.topLevelItemCount() == 0
    assert "no models" in page._status.text()
    page.deleteLater()


def test_the_picker_offers_the_scope_plus_the_running_model(agent_dir):
    from kraken.shell.workspace_view import _offered

    pi_config.save_enabled_models(["anthropic/claude-opus-4-8"])
    assert [m["id"] for m in _offered(MODELS, "anthropic", "claude-opus-4-8")] == [
        "claude-opus-4-8"
    ]
    # The session is on a model that has since been unchecked: it stays on the
    # list, or the one model in use would be the one you cannot switch back to.
    offered = _offered(MODELS, "openrouter", "moonshotai/kimi-k3")
    assert [m["id"] for m in offered] == ["moonshotai/kimi-k3", "claude-opus-4-8"]


# An aggregator's catalogue: every model named vendor/model, several vendors.
ROUTED = [
    {"provider": "openrouter", "id": "moonshotai/kimi-k3", "name": "Kimi K3"},
    {"provider": "openrouter", "id": "moonshotai/kimi-k2.6", "name": "Kimi K2.6"},
    {"provider": "openrouter", "id": "zai/glm-5.1", "name": "GLM 5.1"},
    {"provider": "anthropic", "id": "claude-opus-4-8", "name": "Claude Opus"},
]


@pytest.fixture
def routed(agent_dir, qapp):
    page = ModelsPage(lambda callback: callback(ROUTED))
    yield page
    page.deleteLater()


def test_an_aggregator_is_split_by_the_vendor_it_routes_to(routed):
    openrouter = group(routed, "openrouter")
    assert [
        openrouter.child(i).data(0, _REF) for i in range(openrouter.childCount())
    ] == ["moonshotai", "zai"]
    # The counts on a row are what is under it at any depth, checked against
    # held, since a closed row is often all you see.
    assert openrouter.text(0).endswith("3 of 3 models")
    assert openrouter.child(0).text(0).endswith("2 of 2 models")
    # A vendor's models drop the prefix the row above them already says.
    assert openrouter.child(0).child(0).text(0).startswith("kimi-k3")
    # One vendor is not worth a level: anthropic's models stay directly under it.
    assert group(routed, "anthropic").child(0).childCount() == 0


def test_a_vendor_switches_only_its_own_models(routed):
    openrouter = group(routed, "openrouter")
    openrouter.child(0).setCheckState(0, Qt.CheckState.Unchecked)
    assert scope() == ["openrouter/zai/glm-5.1", "anthropic/claude-opus-4-8"]
    # The counts follow the change, at every level above it.
    assert openrouter.text(0).endswith("1 of 3 models")
    assert openrouter.child(0).text(0).endswith("0 of 2 models")
    # The provider above follows what is left under it, two levels down.
    assert openrouter.checkState(0) == Qt.CheckState.PartiallyChecked
    assert openrouter.child(0).child(0).checkState(0) == Qt.CheckState.Unchecked
    openrouter.setCheckState(0, Qt.CheckState.Checked)
    assert scope() == []
    assert openrouter.child(0).child(1).checkState(0) == Qt.CheckState.Checked


def test_a_vendor_can_be_filtered_for_by_name(routed):
    routed._search.setText("moonshotai")
    openrouter = group(routed, "openrouter")
    assert not openrouter.isHidden()
    assert not openrouter.child(0).isHidden()
    assert openrouter.child(0).isExpanded()
    # The vendor is not in the row's own text any more, but the model still
    # answers to it, and what does not match is out of the way.
    assert not openrouter.child(0).child(0).isHidden()
    assert openrouter.child(1).isHidden()
    assert group(routed, "anthropic").isHidden()


def test_a_nested_model_is_checked_from_the_stored_scope(agent_dir, qapp):
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["openrouter/moonshotai/kimi-k3"]})
    )
    page = ModelsPage(lambda callback: callback(ROUTED))
    openrouter = group(page, "openrouter")
    moonshotai = openrouter.child(0)
    # The reference is the model's, whatever depth the tree shows it at.
    assert moonshotai.child(0).checkState(0) == Qt.CheckState.Checked
    assert moonshotai.child(1).checkState(0) == Qt.CheckState.Unchecked
    assert moonshotai.checkState(0) == Qt.CheckState.PartiallyChecked
    assert openrouter.checkState(0) == Qt.CheckState.PartiallyChecked
    assert group(page, "anthropic").checkState(0) == Qt.CheckState.Unchecked
    page.deleteLater()


def test_the_fallback_rows_are_counted_as_what_they_are(agent_dir, qapp):
    pi_config.settings_path().write_text(
        json.dumps({pi_config.ENABLED_MODELS: ["anthropic/*", "openai/gpt-5.5"]})
    )
    page = ModelsPage(lambda callback: callback([]))
    # Patterns, not models: counting them as models would count the wrong thing,
    # since one pattern can stand for a provider's whole catalogue.
    assert group(page, "settings.json").text(0).endswith("2 of 2 entries")
    page.deleteLater()
