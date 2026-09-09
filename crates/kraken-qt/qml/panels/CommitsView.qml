import QtQuick
import QtQuick.Controls
import "../common"

// The Commits tab of the Git pane: the repo's commit graph, newest first.
// GitPanel is what puts it on screen.
//
// Each row arrives from the bridge as one piece of rich text, because a row is
// not one colour — the hash is tinted for whether the commit is on the main line
// and the graph columns are not, and a per-row layout here would have to know
// that rule a second time.
Item {
    id: panel

    // Nothing to graph: no commits yet, or not a repository at all.
    Text {
        anchors { fill: parent; margins: 12 }
        visible: Git.message !== ""
        text: Git.message
        color: Git.message_color
        font.family: Theme.mono_family
        font.pixelSize: 13
        wrapMode: Text.Wrap
    }

    ListView {
        id: list
        anchors { fill: parent; margins: 10 }
        visible: Git.message === ""
        clip: true
        model: Git.rows
        boundsBehavior: Flickable.StopAtBounds

        ScrollBar.vertical: ThinScrollBar {}

        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index

            // A pure graph line like `|/` carries no commit, so there is
            // nothing on it to hover, click or copy.
            readonly property bool actionable: modelData.short_hash !== ""

            width: list.width
            height: 17
            radius: 4
            color: (rowMouse.containsMouse && actionable) ? Theme.colors.hover
                                                          : "transparent"

            Text {
                anchors {
                    left: parent.left; right: parent.right
                    leftMargin: 4; rightMargin: 12
                    verticalCenter: parent.verticalCenter
                }
                textFormat: Text.RichText
                text: row.modelData.html
                font.family: Theme.mono_family
                font.pixelSize: 13
                elide: Text.ElideRight
            }

            MouseArea {
                id: rowMouse
                anchors.fill: parent
                hoverEnabled: true
                enabled: row.actionable
                acceptedButtons: Qt.RightButton
                cursorShape: Qt.ArrowCursor
                onClicked: rowMenu.popup()

                ToolTipLabel {
                    text: row.modelData.tooltip
                    visible: rowMouse.containsMouse && row.modelData.tooltip !== ""
                }
            }

            Menu {
                id: rowMenu
                MenuItem {
                    text: qsTr("Copy hash")
                    onTriggered: clipboard.copy(row.modelData.full_hash)
                }
                MenuItem {
                    text: qsTr("Check out")
                    enabled: !Git.action_busy
                    onTriggered: Git.checkout(row.modelData.short_hash)
                }
            }
        }
    }

    Clipboard { id: clipboard }

    // git refused the checkout — its own words, not a summary of them.
    Dialog {
        id: refused
        property string message
        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Checkout failed")
        standardButtons: Dialog.Ok
        // Sized here rather than by its content: a Dialog takes its implicit
        // width from what it holds, and content measured back off the dialog
        // closes that into a loop.
        implicitWidth: 420

        Text {
            width: parent.width
            wrapMode: Text.Wrap
            text: refused.message
            color: Theme.colors.text
            font.family: Theme.mono_family
            font.pixelSize: 13
        }
    }

    // ---- Wiring ---------------------------------------------------------------

    // Behind the Changes tab this is invisible, so the graph is neither drawn
    // nor re-read; coming back to it is what asks git again.
    onVisibleChanged: if (visible) Git.refresh()

    Binding { target: Git; property: "theme"; value: Theme.name }

    Connections {
        target: Git
        function onCheckout_failed(message) {
            refused.message = message
            refused.open()
        }
        function onBranch_changed() { if (panel.visible) Git.refresh() }
    }
}
