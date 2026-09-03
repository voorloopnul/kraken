import QtQuick
import "../common"

// One workspace tile: its three letters on the hue that workspace keeps, pale
// while another workspace is current and in full colour once it is.
//
// It also blinks a dot in its top-right corner while that workspace has an
// agent running, so a session working in the background stays visible even when
// the workspace is not on screen.
Item {
    id: tile

    property var entry
    readonly property bool current: entry ? entry.current : false
    readonly property bool active: entry ? entry.active : false

    signal clicked()
    signal menuRequested()

    implicitWidth: 30
    implicitHeight: 30

    Rectangle {
        anchors.fill: parent
        radius: 8
        color: App.tile_color(tile.entry ? tile.entry.key : "", Theme.name,
                              tile.current, mouse.containsMouse)

        // A remote workspace keeps an accent bar down its left: the hue says
        // which folder, and the bar says it is not on this machine.
        Rectangle {
            visible: tile.entry ? tile.entry.remote : false
            anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
            width: 2
            topLeftRadius: 2
            bottomLeftRadius: 2
            color: Theme.colors.accent
        }

        Text {
            anchors.centerIn: parent
            text: tile.entry ? tile.entry.label : ""
            color: "#ffffff"
            font.family: Theme.sans_family
            font.pixelSize: 11
            font.weight: Font.DemiBold
        }
    }

    Rectangle {
        visible: tile.active && blink.on
        anchors { right: parent.right; top: parent.top; margins: 2 }
        width: 8; height: 8; radius: 4
        color: App.indicator_color(Theme.name)
    }

    QtObject {
        id: blink
        property bool on: true
    }

    Timer {
        interval: 600
        repeat: true
        running: tile.active
        onTriggered: blink.on = !blink.on
        onRunningChanged: if (!running) blink.on = true
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: function (event) {
            if (event.button === Qt.RightButton)
                tile.menuRequested()
            else
                tile.clicked()
        }
        ToolTipLabel {
            text: tile.entry ? tile.entry.tooltip : ""
            visible: mouse.containsMouse
        }
    }
}
