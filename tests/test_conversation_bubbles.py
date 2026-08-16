"""User-message bubbles follow the warm light-theme mockup."""

from kraken.chat.conversation import _PALETTE


def test_light_user_bubble_uses_the_mockup_colors():
    colors = _PALETTE["light"]

    assert colors["user_bg"] == "#f1efea"
    assert colors["user_border"] == "#e1ded8"
