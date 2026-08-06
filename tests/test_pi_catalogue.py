"""Asking pi for its model list, and always getting an answer.

The Models page waits on this fetch, so the one thing that must not happen is
silence: a pi that dies takes its pending callbacks with it (`PiAgent` clears
them on exit), and a page told nothing would sit on "asking…" for the rest of
the run. `deliver_once` is what turns that into an empty answer.
"""

import pytest

from kraken.agent import pi_catalogue


@pytest.fixture
def quick(monkeypatch):
    """The timeout, shortened so a test can outlast it."""
    monkeypatch.setattr(pi_catalogue, "_TIMEOUT_MS", 60)


def test_the_answer_is_delivered_once(qapp, quick, settle):
    answers = []
    deliver = pi_catalogue.deliver_once(answers.append)
    deliver([{"id": "a"}])
    deliver([{"id": "b"}])
    settle(150)
    # The second call is ignored, and so is the timeout that follows a real
    # answer — a page rebuilt from an empty list after it had the models would
    # be the fetch undoing itself.
    assert answers == [[{"id": "a"}]]


def test_an_answer_that_never_comes_is_an_empty_one(qapp, quick, settle):
    answers = []
    pi_catalogue.deliver_once(answers.append)
    assert answers == []
    settle(150)
    assert answers == [[]]
