import QtQuick
import "../common"
import "../settings"

// The question a copy stops on: something at the other end is already called
// this.
//
// It stops the copy rather than deciding for it. Replacing quietly destroys
// work; renaming quietly leaves a `report 2.txt` that nobody asked for and
// nobody notices until the wrong one gets sent. Both answers lose something, so
// both are on screen and neither is the default.
//
// Over the whole window rather than inside the Files pane: the pane is a column
// three hundred pixels wide, and a question about deleting someone's file is not
// something to fit into it.
Item {
    id: dialog

    visible: Files.collision_open
    // Above the panels and the preview sheet — it is the only thing on screen
    // that is waiting for an answer.
    z: 110

    Rectangle {
        anchors.fill: parent
        color: Theme.name === "dark" ? Qt.rgba(0, 0, 0, 0.45)
                                     : Qt.rgba(0, 0, 0, 0.28)

        // Clicking away is a cancel. It is the answer that changes nothing,
        // which is the only one safe to give by accident.
        MouseArea {
            anchors.fill: parent
            onClicked: Files.resolve_collision("cancel")
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 120, 440)
        height: column.implicitHeight + 40
        radius: 10
        color: Theme.colors.card
        border.width: 1
        border.color: Theme.colors.card_border

        MouseArea { anchors.fill: parent }

        Column {
            id: column
            anchors {
                left: parent.left; right: parent.right
                verticalCenter: parent.verticalCenter
                leftMargin: 20; rightMargin: 20
            }
            spacing: 8

            Text {
                width: parent.width
                text: Files.collision_message
                color: Theme.colors.text
                font.family: Theme.sans_family
                font.pixelSize: 12
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
            }

            Text {
                width: parent.width
                text: qsTr("Replacing cannot be undone.")
                color: Files.dim_color
                font.family: Theme.sans_family
                font.pixelSize: 11
                wrapMode: Text.Wrap
            }

            Item { width: 1; height: 6 }

            Row {
                anchors.right: parent.right
                spacing: 6

                SettingsChip {
                    text: qsTr("Cancel")
                    onClicked: Files.resolve_collision("cancel")
                }
                SettingsChip {
                    text: qsTr("Keep both")
                    onClicked: Files.resolve_collision("keep_both")
                }
                SettingsChip {
                    text: qsTr("Replace")
                    danger: true
                    onClicked: Files.resolve_collision("replace")
                }
            }
        }
    }

    // Escape cancels, which is why the dialog takes focus while it is up.
    focus: visible
    Keys.onEscapePressed: Files.resolve_collision("cancel")
    onVisibleChanged: if (visible) forceActiveFocus()
}
