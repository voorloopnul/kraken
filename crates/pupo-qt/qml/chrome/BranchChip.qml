import QtQuick
import "../common"

// The branch switcher under the workspace path: shows the current branch, and
// clicking it opens the repo's local branches.
Rectangle {
    id: chip

    property string branch: ""
    signal clicked()

    readonly property color foreground: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"

    implicitWidth: row.implicitWidth + 10
    implicitHeight: row.implicitHeight + 2
    radius: 5
    color: mouse.containsMouse ? Theme.colors.hover : "transparent"

    Row {
        id: row
        anchors.centerIn: parent
        spacing: 3

        Image {
            anchors.verticalCenter: parent.verticalCenter
            width: 12; height: 12
            sourceSize: Qt.size(24, 24)
            source: Theme.icon("git-branch", chip.foreground)
        }
        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: chip.branch
            color: chip.foreground
            font.family: Theme.sans_family
            font.pixelSize: 11
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: chip.clicked()
        ToolTipLabel { text: qsTr("Switch branch"); visible: mouse.containsMouse }
    }
}
