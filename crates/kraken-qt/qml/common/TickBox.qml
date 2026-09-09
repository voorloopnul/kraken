import QtQuick

// A small square that is either ticked or not.
//
// Not `QtQuick.Controls`' CheckBox: that one brings a label, an indicator and a
// padding scheme of its own, and every place we use one is a row that has
// already decided where its text goes. What is left is the square, the tick and
// the click — which is all a checked file needs.
Item {
    id: box

    property bool checked: false
    /// Some but not all of what the box speaks for — the Select All box over a
    /// list where only a few rows are ticked.
    property bool partial: false
    property bool enabled: true
    property string tooltip

    signal toggled(bool on)

    implicitWidth: 13
    implicitHeight: 13

    Rectangle {
        anchors.fill: parent
        radius: 3
        color: box.checked || box.partial ? Theme.colors.accent : "transparent"
        border.width: 1
        border.color: box.checked || box.partial ? Theme.colors.accent
                                                 : Theme.colors.card_border
        opacity: box.enabled ? 1.0 : 0.45

        Text {
            anchors.centerIn: parent
            visible: box.checked || box.partial
            text: box.partial ? "−" : "✓"
            color: Theme.colors.accent_on
            font.family: Theme.mono_family
            font.pixelSize: 9
        }
    }

    // A 13-pixel target is a hard one to hit, so the area that takes the click
    // is larger than the square that shows the answer.
    MouseArea {
        id: mouse
        anchors { fill: parent; margins: -4 }
        hoverEnabled: true
        enabled: box.enabled
        cursorShape: Qt.PointingHandCursor
        onClicked: box.toggled(!box.checked)
    }

    ToolTipLabel {
        text: box.tooltip
        visible: mouse.containsMouse && box.tooltip !== ""
    }
}
