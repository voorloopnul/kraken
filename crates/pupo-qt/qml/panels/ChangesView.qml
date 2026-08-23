import QtQuick
import QtQuick.Controls
import "../common"

// The Changes tab of the Git pane: what has changed in this workspace's repo
// since the last commit, file by file. GitPanel is what puts it on screen.
//
// One row per file with its own added and removed counts, not a split between
// staged and unstaged — the question the tab answers is "what has the agent
// been doing", and a change that is half staged is still one change. Every row
// is a click away from the full diff (see DiffViewer.qml).
//
// The colours are the bridge's rather than this file's: a status letter, a
// deleted path and a zero count each have a colour that means something, and
// working that rule out twice is how the two copies of it drift.
Item {
    id: panel

    Column {
        anchors { fill: parent; margins: 10 }
        spacing: 6

        // Totals, and the only place the empty and failed states are said. No
        // placeholder row is faked into the list: a pane with one row in it that
        // is not a file reads as a file.
        Text {
            id: summary
            width: parent.width
            textFormat: Text.RichText
            text: Diff.summary
            font.family: Theme.mono_family
            font.pixelSize: 13
            wrapMode: Text.Wrap
        }

        ListView {
            id: list
            width: parent.width
            height: parent.height - summary.height - parent.spacing
            clip: true
            model: Diff.files
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}

            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index

                width: list.width
                height: 20
                radius: 4
                color: rowMouse.containsMouse ? Theme.colors.hover : "transparent"

                Text {
                    id: letter
                    anchors { left: parent.left; leftMargin: 4; verticalCenter: parent.verticalCenter }
                    width: 12
                    text: row.modelData.letter
                    color: row.modelData.letter_color
                    font.family: Theme.mono_family
                    font.pixelSize: 13
                    font.weight: Font.DemiBold
                }

                Text {
                    anchors {
                        left: letter.right; leftMargin: 4
                        right: counts.left; rightMargin: 8
                        verticalCenter: parent.verticalCenter
                    }
                    text: row.modelData.path
                    color: row.modelData.path_color
                    font.family: Theme.mono_family
                    font.pixelSize: 13
                    // Eat into the leading directories: a path's tail is the
                    // informative end, and hiding the filename hides the row.
                    elide: Text.ElideLeft
                }

                Row {
                    id: counts
                    // Clear of the scrollbar, which overlays the list's right
                    // edge rather than taking a column out of it.
                    anchors { right: parent.right; rightMargin: 12; verticalCenter: parent.verticalCenter }
                    spacing: 6

                    Text {
                        text: row.modelData.adds
                        color: row.modelData.adds_color
                        font.family: Theme.mono_family
                        font.pixelSize: 13
                    }
                    Text {
                        text: row.modelData.dels
                        color: row.modelData.dels_color
                        font.family: Theme.mono_family
                        font.pixelSize: 13
                    }
                }

                MouseArea {
                    id: rowMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.RightButton
                    onClicked: function (event) {
                        if (event.button === Qt.RightButton) {
                            rowMenu.popup()
                            return
                        }
                        Diff.open_file(row.index)
                    }

                    ToolTipLabel {
                        text: row.modelData.tooltip
                        visible: rowMouse.containsMouse && row.modelData.tooltip !== ""
                    }
                }

                Menu {
                    id: rowMenu
                    MenuItem {
                        text: qsTr("Copy path")
                        onTriggered: clipboard.copy(Diff.path_at(row.index))
                    }
                    MenuItem {
                        text: qsTr("Open diff")
                        onTriggered: Diff.open_file(row.index)
                    }
                }
            }
        }
    }

    Clipboard { id: clipboard }

    // ---- Wiring ---------------------------------------------------------------

    // Refreshed on first sight rather than on a timer: git is a subprocess (an
    // SSH round trip on a remote workspace), and a tab nobody is looking at is
    // not worth one. Behind the Commits tab this is invisible and so silent,
    // and coming back to it is what re-reads the repository.
    onVisibleChanged: if (visible) Diff.refresh()

    Binding { target: Diff; property: "theme"; value: Theme.name }

    // A commit or a checkout changes what "since the last commit" means, so the
    // tab's whole answer changes with HEAD.
    Connections {
        target: Git
        function onBranch_changed() { if (panel.visible) Diff.refresh() }
    }
}
