import QtQuick
import "../common"

// The strip down the window's right edge, IDE tool-window style: the panel
// toggles at the top and the screenshot action pinned to the bottom.
//
// A button whose panel is open is filled with the accent and carries a white
// glyph — the one thing in this strip that is not grey, because "which panels
// are open" is the only question the strip answers.
Rectangle {
    id: bar

    property int cornerRadius: 0
    signal screenshotRequested()

    width: 40
    color: Theme.colors.sidebar
    bottomRightRadius: bar.cornerRadius

    Rectangle {
        anchors { top: parent.top; bottom: parent.bottom; left: parent.left }
        width: 1
        color: Theme.colors.card_border
    }

    Column {
        anchors { top: parent.top; left: parent.left; right: parent.right }
        anchors.margins: 4
        anchors.topMargin: 8
        spacing: 6

        Repeater {
            model: [
                { side: "right",   glyph: "square-terminal", tip: qsTr("Terminal Panel") },
                { side: "browser", glyph: "globe",           tip: qsTr("Browser Panel") },
                { side: "diff",    glyph: "diff",            tip: qsTr("Diff Panel") },
                { side: "git",     glyph: "git-branch",      tip: qsTr("Git Panel") }
            ]
            delegate: IconButton {
                required property var modelData
                anchors.horizontalCenter: parent.horizontalCenter
                glyph: modelData.glyph
                tooltip: modelData.tip
                checkable: true
                checked: App.is_panel_visible(modelData.side)
                onClicked: App.set_panel_visible(modelData.side, checked)

                Connections {
                    target: App
                    function onPanels_changed() {
                        checked = App.is_panel_visible(modelData.side)
                    }
                    function onCurrent_changed() {
                        checked = App.is_panel_visible(modelData.side)
                    }
                }
            }
        }
    }

    Column {
        anchors { bottom: parent.bottom; left: parent.left; right: parent.right }
        anchors.margins: 4
        anchors.bottomMargin: 8

        IconButton {
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "camera"
            tooltip: qsTr("Screenshot")
            onClicked: bar.screenshotRequested()
        }
    }
}
