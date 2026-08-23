import QtQuick
import QtQuick.Controls
import "../common"

// One row of the History pane, in either of its two lists.
//
// Lives here rather than inline in LeftPanel because the pinned sessions and
// the recent ones are separate lists over separate models: same row, two
// headings, and only the list it sits in knows which group that is.
Rectangle {
    id: row

    required property var modelData
    /// The pane's own text colours, passed down so the palette is decided in
    /// one place rather than recomputed per row.
    property color titleColor
    property color subtitleColor

    height: rowText.implicitHeight + 24
    radius: 6
    color: modelData.selected ? Theme.colors.accent_soft
         : rowMouse.containsMouse ? Theme.colors.hover
         : "transparent"

    // The status dot: amber while a turn is streaming, green once it has
    // finished with a result nobody has opened yet.
    Rectangle {
        id: dot
        anchors {
            left: parent.left
            leftMargin: pinMark.visible ? 24 : 6
            verticalCenter: parent.verticalCenter
        }
        width: 8; height: 8; radius: 4
        visible: row.modelData.status !== ""
        color: row.modelData.status === "running" ? "#e0a030" : "#2ea043"
    }

    // The pin leads the row, ahead of the status dot. A pinned row keeps this
    // wherever it is shown, so the mark travels with the row rather than being
    // a property of the list it happens to be in.
    Image {
        id: pinMark
        visible: row.modelData.pinned
        anchors { left: parent.left; leftMargin: 6; verticalCenter: parent.verticalCenter }
        width: 14
        height: 14
        // Rendered at twice the logical size so the strokes stay clean where
        // the desktop is scaled, as IconButton does.
        sourceSize: Qt.size(28, 28)
        smooth: true
        source: Theme.icon("pin", row.subtitleColor)
    }

    Column {
        id: rowText
        anchors {
            left: parent.left
            right: parent.right
            verticalCenter: parent.verticalCenter
            // Clear of whichever markers are showing: 10 on its own, plus the
            // room the pin and the dot each take when they are there.
            leftMargin: 10 + (pinMark.visible ? 18 : 0) + (dot.visible ? 10 : 0)
            rightMargin: 10
        }
        spacing: 2

        Text {
            width: parent.width
            text: row.modelData.title
            color: row.modelData.selected ? Theme.colors.accent_text : row.titleColor
            font.family: Theme.sans_family
            font.pixelSize: 14
            wrapMode: Text.Wrap
            maximumLineCount: 2
            elide: Text.ElideRight
        }
        Text {
            width: parent.width
            text: row.modelData.subtitle
            color: row.subtitleColor
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
                // Only a persisted session has a file to act on; a live
                // in-flight row has nothing to pin or archive.
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
        // One item, not two: a pinned row offers the way back rather than
        // offering a pin that is already set.
        MenuItem {
            text: row.modelData.pinned ? qsTr("Unpin") : qsTr("Pin")
            onTriggered: row.modelData.pinned ? History.unpin(row.modelData.key)
                                              : History.pin(row.modelData.key)
        }
        MenuSeparator {}
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

        // Sized here rather than by its content: a Dialog takes its implicit
        // width from what it holds, and content measured back off the dialog
        // closes that into a loop.
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
