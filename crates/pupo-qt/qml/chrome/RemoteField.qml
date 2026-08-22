import QtQuick
import "../settings"

// One labelled field of the remote dialog, with an optional Browse beside it.
//
// The label column is fixed so the fields line up down the form; the field takes
// what is left, which is where a hostname or an absolute path needs the room.
Item {
    id: field

    property string label
    property alias text: input.text
    property alias placeholderText: input.placeholderText
    property bool browsable: false

    signal browse()

    width: parent ? parent.width : 0
    height: 26

    Text {
        anchors { left: parent.left; verticalCenter: parent.verticalCenter }
        width: 96
        text: field.label
        color: Theme.chat_colors.dim
        font.family: Theme.sans_family
        font.pixelSize: 12
    }

    SettingsField {
        id: input
        anchors {
            left: parent.left; leftMargin: 100
            right: browseButton.visible ? browseButton.left : parent.right
            rightMargin: browseButton.visible ? 6 : 0
            verticalCenter: parent.verticalCenter
        }
        width: undefined
    }

    SettingsChip {
        id: browseButton
        anchors { right: parent.right; verticalCenter: parent.verticalCenter }
        visible: field.browsable
        text: qsTr("Browse…")
        onClicked: field.browse()
    }
}
