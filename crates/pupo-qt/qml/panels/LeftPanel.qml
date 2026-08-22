import QtQuick
import QtQuick.Controls
import "../common"

// The History pane: the Pi sessions recorded for this workspace folder, plus
// whatever is running right now.
//
// The one panel that is not a card. It paints its own background out to its
// edges, so it reads as part of the window rather than as something resting on
// it; the hairline separating it from the conversation is the dock's divider,
// the same one between any two panels, and a border here would double it.
Rectangle {
    id: panel
    color: Theme.colors.sidebar

    // History rows use the proportional face: a session's title is prose, and a
    // mono grid makes a list of sentences read like a table of data.
    readonly property color rowText: Theme.name === "dark" ? "#c8cad0" : "#383a42"
    readonly property color rowSubtitle: Theme.name === "dark" ? "#7a7d85" : "#8e8b86"

    Column {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 10

        Rectangle {
            id: newButton
            width: parent.width
            height: 30
            radius: 6
            color: newMouse.containsMouse ? Theme.colors.hover : Theme.colors.header
            border.width: 1
            border.color: Theme.colors.card_border

            Text {
                anchors.centerIn: parent
                text: "＋  " + qsTr("New Session")
                color: newMouse.containsMouse ? Theme.colors.text : panel.rowSubtitle
                font.family: Theme.mono_family
                font.pixelSize: 12
            }

            MouseArea {
                id: newMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: History.request_new_session()
            }
        }

        ListView {
            id: list
            width: parent.width
            height: parent.height - newButton.height - parent.spacing
            clip: true
            spacing: 2
            model: History.sessions
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}

            // Nothing recorded and nothing running: say so rather than leaving
            // a blank column that reads as a panel that failed to load.
            Text {
                anchors.centerIn: parent
                visible: list.count === 0
                text: qsTr("No previous sessions")
                color: panel.rowSubtitle
                font.family: Theme.sans_family
                font.pixelSize: 13
            }

            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index

                width: list.width
                height: rowText.implicitHeight + 24
                radius: 6
                color: modelData.selected ? Theme.colors.accent_soft
                     : rowMouse.containsMouse ? Theme.colors.hover
                     : "transparent"

                // The status dot: amber while a turn is streaming, green once
                // it has finished with a result nobody has opened yet.
                Rectangle {
                    id: dot
                    anchors { left: parent.left; leftMargin: 6; verticalCenter: parent.verticalCenter }
                    width: 8; height: 8; radius: 4
                    visible: row.modelData.status !== ""
                    color: row.modelData.status === "running" ? "#e0a030" : "#2ea043"
                }

                Column {
                    id: rowText
                    anchors {
                        left: parent.left
                        right: parent.right
                        verticalCenter: parent.verticalCenter
                        leftMargin: row.modelData.status !== "" ? 20 : 10
                        rightMargin: 10
                    }
                    spacing: 2

                    Text {
                        width: parent.width
                        text: row.modelData.title
                        color: row.modelData.selected ? Theme.colors.accent_text
                                                      : panel.rowText
                        font.family: Theme.sans_family
                        font.pixelSize: 13
                        wrapMode: Text.Wrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        text: row.modelData.subtitle
                        color: panel.rowSubtitle
                        font.family: Theme.sans_family
                        font.pixelSize: 11
                        elide: Text.ElideRight
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
                            // Only a persisted session has a file to act on; a
                            // live in-flight row has nothing to archive.
                            if (row.modelData.session_id !== "")
                                rowMenu.popup()
                            return
                        }
                        History.activate(row.modelData.key)
                    }
                    ToolTipLabel {
                        text: row.modelData.tooltip
                        visible: rowMouse.containsMouse && row.modelData.tooltip !== ""
                    }
                }

                Menu {
                    id: rowMenu
                    MenuItem {
                        text: qsTr("Archive")
                        onTriggered: History.archive(row.modelData.key)
                    }
                    MenuItem {
                        text: qsTr("Delete")
                        onTriggered: confirmDelete.open()
                    }
                }

                Dialog {
                    id: confirmDelete
                    parent: Overlay.overlay
                    anchors.centerIn: parent
                    modal: true
                    title: qsTr("Delete session")
                    standardButtons: Dialog.Yes | Dialog.No
                    onAccepted: History.remove(row.modelData.key)

                    // Sized here rather than by its content: a Dialog takes its
                    // implicit width from what it holds, and content measured
                    // back off the dialog closes that into a loop.
                    implicitWidth: 360

                    Text {
                        width: parent.width
                        wrapMode: Text.Wrap
                        text: qsTr("Permanently delete this session? This cannot be undone.")
                        color: Theme.colors.text
                        font.family: Theme.sans_family
                        font.pixelSize: 12
                    }
                }
            }
        }
    }
}
