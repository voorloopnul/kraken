import QtQuick
import QtQuick.Controls
import "../common"

// Every model pi offers, with a checkbox each.
//
// The checked set is pi's own `enabledModels` in `~/.pi/agent/settings.json` —
// the setting behind its `--models` flag — rather than a preference of ours, so
// the scope chosen here is the scope pi starts a session with and the one its
// own picker cycles through. The catalogue comes from pi as well: a page whose
// checkboxes came from somewhere else would be a page about a different list.
Item {
    id: page

    Column {
        anchors { fill: parent; leftMargin: 20; rightMargin: 20; topMargin: 4 }
        spacing: 0

        SettingsSection {
            width: parent.width
            text: qsTr("Model Scope")
            first: true
        }

        SettingsNote {
            width: parent.width
            text: qsTr("Checked models are the ones pi offers — here, in the "
                       + "composer's picker, and in its own. Leaving every model "
                       + "checked saves no scope at all, so a model added later "
                       + "shows up on its own.")
        }

        Item {
            width: parent.width
            height: 34

            SettingsField {
                id: filter
                anchors { left: parent.left; right: buttons.left; rightMargin: 8
                          verticalCenter: parent.verticalCenter }
                placeholderText: qsTr("Filter models")
                onTextChanged: Settings.filter_models(text)
            }

            Row {
                id: buttons
                anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                spacing: 6
                SettingsChip { text: qsTr("All"); onClicked: Settings.set_all_models(true) }
                SettingsChip { text: qsTr("None"); onClicked: Settings.set_all_models(false) }
                SettingsChip { text: qsTr("Save"); onClicked: Settings.save_models() }
            }
        }

        // The tree. Tall enough to browse a provider's catalogue without
        // swallowing the page it sits on; it scrolls inside that.
        Rectangle {
            width: parent.width
            height: parent.height - y - status.height - 12
            radius: 6
            color: Theme.colors.card
            border.width: 1
            border.color: Theme.colors.card_border

            Text {
                anchors.centerIn: parent
                visible: Settings.models_loading
                text: qsTr("Asking pi for its model list…")
                color: Theme.chat_colors.dim
                font.family: Theme.sans_family
                font.pixelSize: 12
            }

            ListView {
                id: tree
                anchors { fill: parent; margins: 6 }
                visible: !Settings.models_loading
                clip: true
                model: Settings.model_rows
                boundsBehavior: Flickable.StopAtBounds

                ScrollBar.vertical: ThinScrollBar {}

                delegate: Rectangle {
                    id: node
                    required property var modelData

                    width: tree.width
                    height: 22
                    radius: 4
                    color: nodeMouse.containsMouse ? Theme.colors.hover : "transparent"

                    // Only a group has anything to fold, so only a group draws
                    // the triangle — a leaf with a blank where one would be
                    // reads as a group that failed to load.
                    Text {
                        id: twisty
                        anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                        x: 4 + node.modelData.depth * 14
                        width: 12
                        visible: node.modelData.group
                        text: node.modelData.expanded ? "▾" : "▸"
                        color: Theme.chat_colors.dim
                        font.family: Theme.mono_family
                        font.pixelSize: 10

                        MouseArea {
                            anchors.fill: parent
                            cursorShape: Qt.PointingHandCursor
                            onClicked: Settings.expand_model_group(
                                node.modelData.index, !node.modelData.expanded)
                        }
                    }

                    // Three states, because a group is as often half checked as
                    // it is either of the other two.
                    Rectangle {
                        id: box
                        anchors { verticalCenter: parent.verticalCenter }
                        x: twisty.x + 16
                        width: 13
                        height: 13
                        radius: 3
                        color: node.modelData.state === "off" ? "transparent"
                                                              : Theme.colors.accent
                        border.width: 1
                        border.color: node.modelData.state === "off"
                                      ? Theme.colors.card_border : Theme.colors.accent

                        Text {
                            anchors.centerIn: parent
                            text: node.modelData.state === "partial" ? "–" : "✓"
                            visible: node.modelData.state !== "off"
                            color: Theme.colors.accent_on
                            font.family: Theme.mono_family
                            font.pixelSize: 9
                        }
                    }

                    Text {
                        anchors {
                            left: box.right; leftMargin: 8
                            right: parent.right; rightMargin: 12
                            verticalCenter: parent.verticalCenter
                        }
                        text: node.modelData.label
                        color: node.modelData.group ? Theme.colors.text
                                                    : Theme.chat_colors.dim
                        font.family: node.modelData.group ? Theme.mono_family
                                                          : Theme.sans_family
                        font.pixelSize: 11
                        elide: Text.ElideRight
                    }

                    MouseArea {
                        id: nodeMouse
                        anchors.fill: parent
                        anchors.leftMargin: box.x
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: Settings.toggle_model(node.modelData.index)
                    }
                }
            }
        }

        SettingsNote {
            id: status
            width: parent.width
            text: Settings.models_status
        }
    }
}
