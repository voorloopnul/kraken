import QtQuick
import QtQuick.Controls
import "../common"

// What Pupo is holding in memory, process by process.
//
// The number in the title bar is the whole tree's, and this is what is behind
// it: children are what an app like this leaks — an agent per workspace, a shell
// per terminal, an ssh client per remote command — so the table lists this
// process first and everything it started after it.
//
// An overlay for the same reasons the other two are (see SettingsWindow.qml).
Item {
    id: dialog

    property bool shown: false

    visible: shown
    z: 95

    function open() {
        App.sample_processes()
        shown = true
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.name === "dark" ? Qt.rgba(0, 0, 0, 0.45)
                                     : Qt.rgba(0, 0, 0, 0.28)
        MouseArea {
            anchors.fill: parent
            onClicked: dialog.shown = false
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 120, 620)
        height: Math.min(parent.height - 120, 460)
        radius: 10
        color: Theme.colors.card
        border.width: 1
        border.color: Theme.colors.card_border

        MouseArea { anchors.fill: parent }

        Rectangle {
            id: bar
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 36
            topLeftRadius: card.radius
            topRightRadius: card.radius
            color: Theme.colors.header

            Rectangle {
                anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                height: 1
                color: Theme.colors.card_border
            }

            Text {
                anchors.centerIn: parent
                text: qsTr("Process Memory")
                color: Theme.colors.text
                font.family: Theme.mono_family
                font.pixelSize: 12
                font.weight: Font.DemiBold
            }

            IconButton {
                anchors { right: parent.right; rightMargin: 6; verticalCenter: parent.verticalCenter }
                glyph: "x"
                tooltip: qsTr("Close")
                onClicked: dialog.shown = false
            }
        }

        Item {
            id: head
            anchors { left: parent.left; right: parent.right; top: bar.bottom }
            anchors.margins: 16
            height: 26

            Text {
                anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                text: App.process_summary
                color: Theme.chat_colors.dim
                font.family: Theme.sans_family
                font.pixelSize: 11
            }

            TextButton {
                anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                text: qsTr("Refresh")
                onClicked: App.sample_processes()
            }
        }

        Rectangle {
            anchors {
                left: parent.left; right: parent.right
                top: head.bottom; bottom: parent.bottom
                leftMargin: 16; rightMargin: 16; bottomMargin: 16
            }
            radius: 7
            color: Theme.colors.header
            border.width: 1
            border.color: Theme.colors.card_border

            ListView {
                id: table
                anchors { fill: parent; margins: 6 }
                clip: true
                model: App.processes
                boundsBehavior: Flickable.StopAtBounds

                ScrollBar.vertical: ThinScrollBar {}

                delegate: Item {
                    id: line
                    required property var modelData
                    required property int index

                    width: table.width
                    height: 22

                    Text {
                        anchors { left: parent.left; leftMargin: 8; right: pid.left
                                  rightMargin: 8; verticalCenter: parent.verticalCenter }
                        text: line.modelData.name
                        // This process is the app; the rest are programs it
                        // started, and the difference is worth seeing at a
                        // glance in a table about what they cost.
                        color: line.index === 0 ? Theme.colors.text
                                                : Theme.chat_colors.dim
                        font.family: Theme.mono_family
                        font.pixelSize: 11
                        elide: Text.ElideRight
                    }

                    // Both numbers right-aligned in fixed columns, so the digits
                    // line up down the table rather than drifting with the name
                    // beside them.
                    Text {
                        id: pid
                        anchors { right: memory.left; rightMargin: 16; verticalCenter: parent.verticalCenter }
                        width: 60
                        horizontalAlignment: Text.AlignRight
                        text: line.modelData.pid
                        color: Theme.chat_colors.dim
                        font.family: Theme.mono_family
                        font.pixelSize: 11
                    }

                    Text {
                        id: memory
                        anchors { right: parent.right; rightMargin: 14; verticalCenter: parent.verticalCenter }
                        width: 72
                        horizontalAlignment: Text.AlignRight
                        text: line.modelData.memory
                        color: Theme.colors.text
                        font.family: Theme.mono_family
                        font.pixelSize: 11
                    }
                }
            }
        }
    }

    focus: visible
    Keys.onEscapePressed: dialog.shown = false
    onVisibleChanged: if (visible) forceActiveFocus()
}
