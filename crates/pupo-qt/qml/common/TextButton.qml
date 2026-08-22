import QtQuick

// A borderless label that acts like a button.
//
// The composer's footer and the busy row are built from these rather than from
// IconButton: they read as words, not glyphs, and the vendored icon set has no
// mark for "Send" or "Effort" that would be clearer than the word itself.
Item {
    id: control

    property string text
    property string tooltip
    property bool enabled: true
    property color textColor: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
    property color hoverTextColor: Theme.colors.text
    property color disabledColor: Theme.name === "dark" ? "#55575d" : "#b0b2b8"
    property int fontSize: Theme.secondary_font_size

    signal clicked()

    implicitWidth: label.implicitWidth + 12
    implicitHeight: label.implicitHeight + 4

    Rectangle {
        anchors.fill: parent
        radius: 4
        color: mouse.containsMouse && control.enabled ? Theme.colors.hover
                                                      : "transparent"
    }

    Text {
        id: label
        anchors.centerIn: parent
        text: control.text
        color: !control.enabled ? control.disabledColor
             : mouse.containsMouse ? control.hoverTextColor
                                   : control.textColor
        font.family: Theme.mono_family
        font.pixelSize: control.fontSize
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        enabled: control.enabled
        cursorShape: Qt.PointingHandCursor
        onClicked: control.clicked()
    }

    ToolTipLabel {
        text: control.tooltip
        visible: mouse.containsMouse && control.tooltip !== ""
    }
}
