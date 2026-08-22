import QtQuick
import QtQuick.Controls

// The app's one tooltip. Controls' default carries the platform style's
// colours, which on a themed window read as a piece of someone else's chrome.
ToolTip {
    id: tip
    delay: 500
    font.family: Theme.sans_family
    font.pixelSize: 11
    contentItem: Text {
        text: tip.text
        color: Theme.colors.text
        font: tip.font
    }
    background: Rectangle {
        color: Theme.colors.header
        border.color: Theme.colors.card_border
        border.width: 1
        radius: 5
    }
}
