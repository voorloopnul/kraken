import QtQuick
import "../common"

// "Open this folder in another app", as a two-segment control: the app's icon
// on the left, a chevron on the right, one border around both.
//
// The chevron is drawn here rather than left to a menu indicator. It is two
// short lines and a divider, and drawing them is what keeps the control the
// same shape whatever the platform decides a dropdown arrow should look like.
Rectangle {
    id: control

    // Greyed rather than hidden when there is nothing to open: a remote
    // workspace's anchor holds none of the project, so the control stays where
    // it is and stops offering, instead of leaving a hole in the bar.
    property bool enabled: true
    signal clicked()

    readonly property color chrome: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
    // The right-hand segment, measured from the control's own edge so both
    // halves keep their proportions if the size ever changes.
    readonly property real chevronSegment: 23

    implicitWidth: 62
    implicitHeight: 28
    radius: 7
    color: mouse.containsMouse && control.enabled ? Theme.colors.hover : "transparent"
    border.width: 1
    // A disabled control keeps its footprint but drops its outline, so the bar
    // does not read as having a dead button in it.
    border.color: control.enabled ? Theme.colors.card_border : "transparent"
    opacity: control.enabled ? 1 : 0.45

    Image {
        anchors.verticalCenter: parent.verticalCenter
        x: 7
        width: 19
        height: 19
        sourceSize: Qt.size(38, 38)
        source: Theme.icon("square-terminal", control.chrome)
    }

    // The hairline between the two segments.
    Rectangle {
        x: control.width - control.chevronSegment
        y: 4
        width: 1
        height: control.height - 8
        color: control.enabled ? Theme.colors.card_border : "transparent"
    }

    // The chevron: two strokes meeting at the bottom, drawn as thin rotated
    // rectangles so they need no Canvas and no repaint of their own.
    Item {
        width: 8
        height: 6
        anchors.verticalCenter: parent.verticalCenter
        x: control.width - control.chevronSegment / 2 - width / 2

        Rectangle {
            width: 5.5; height: 1.5; radius: 0.75
            color: control.chrome
            x: -0.6; y: 1.6
            rotation: 45
            transformOrigin: Item.Center
        }
        Rectangle {
            width: 5.5; height: 1.5; radius: 0.75
            color: control.chrome
            x: 3.1; y: 1.6
            rotation: -45
            transformOrigin: Item.Center
        }
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: control.enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: if (control.enabled) control.clicked()
        ToolTipLabel {
            text: control.enabled ? qsTr("Open this folder in another app")
                                  : qsTr("A remote workspace has nothing local to open")
            visible: mouse.containsMouse
        }
    }
}
