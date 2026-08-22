import QtQuick

// A small bordered action button — Save, Forget, Sign out.
//
// Bordered rather than borderless like the composer's footer buttons: these
// write to a file the moment they are pressed, and a control with a consequence
// should look like something you press on purpose.
Item {
    id: chip

    property string text
    property bool enabled: true

    signal clicked()

    implicitWidth: label.implicitWidth + 22
    implicitHeight: 24

    Rectangle {
        anchors.fill: parent
        radius: 5
        color: !chip.enabled ? "transparent"
             : mouse.containsMouse ? Theme.colors.hover
                                   : Theme.colors.header
        border.width: 1
        border.color: Theme.colors.card_border
    }

    Text {
        id: label
        anchors.centerIn: parent
        text: chip.text
        color: chip.enabled ? Theme.colors.text : Theme.chat_colors.dim
        font.family: Theme.sans_family
        font.pixelSize: 11
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        enabled: chip.enabled
        cursorShape: Qt.PointingHandCursor
        onClicked: chip.clicked()
    }
}
