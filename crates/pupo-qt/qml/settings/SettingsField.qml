import QtQuick
import QtQuick.Controls

// A one-line text field in the settings window's own dress.
//
// Controls' default carries the platform style's frame, which on a themed
// frameless window reads as a piece of someone else's chrome — the same reason
// the scrollbar is ours (see ThinScrollBar.qml).
TextField {
    id: field

    property bool secret: false

    width: parent ? parent.width : implicitWidth
    echoMode: secret ? TextInput.Password : TextInput.Normal
    color: Theme.colors.text
    placeholderTextColor: Theme.chat_colors.dim
    font.family: Theme.mono_family
    font.pixelSize: 12
    selectByMouse: true
    leftPadding: 8
    rightPadding: 8
    topPadding: 5
    bottomPadding: 5

    background: Rectangle {
        radius: 5
        color: Theme.colors.card
        border.width: 1
        border.color: field.activeFocus ? Theme.colors.accent
                                        : Theme.colors.card_border
    }
}
