import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import "../common"
import "../settings"

// Add or edit a remote workspace: a folder on another machine that pi works in
// over SSH.
//
// The picker at the top offers the machine's own `~/.ssh/config` beside the
// profiles already saved here, because a host worth opening a workspace on is
// usually one ssh already knows about — and retyping a hostname written down two
// directories away is how a typo becomes a workspace that never connects.
//
// Test Connection is the point of the dialog. Everything it asks for is
// something ssh will either accept or not, and finding that out after the
// workspace is in the bar is finding it out too late.
Item {
    id: dialog

    visible: Remotes.open
    z: 96

    Rectangle {
        anchors.fill: parent
        color: Theme.name === "dark" ? Qt.rgba(0, 0, 0, 0.45)
                                     : Qt.rgba(0, 0, 0, 0.28)
        MouseArea {
            anchors.fill: parent
            onClicked: Remotes.hide()
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 120, 520)
        height: body.implicitHeight + bar.height + 32
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
                text: Remotes.editing ? qsTr("Edit Remote Workspace")
                                      : qsTr("Add Remote Workspace")
                color: Theme.colors.text
                font.family: Theme.mono_family
                font.pixelSize: 12
                font.weight: Font.DemiBold
            }

            IconButton {
                anchors { right: parent.right; rightMargin: 6; verticalCenter: parent.verticalCenter }
                glyph: "x"
                tooltip: qsTr("Close")
                onClicked: Remotes.hide()
            }
        }

        Column {
            id: body
            anchors {
                left: parent.left; right: parent.right; top: bar.bottom
                leftMargin: 18; rightMargin: 18; topMargin: 14
            }
            spacing: 8

            // The picker. A field rather than a menu of its own: the list is
            // short, and the entries it offers are only a starting point —
            // every one of them is editable below.
            Item {
                width: parent.width
                height: 26

                Text {
                    anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                    width: 96
                    text: qsTr("Known hosts")
                    color: Theme.chat_colors.dim
                    font.family: Theme.sans_family
                    font.pixelSize: 12
                }

                Flickable {
                    anchors { left: parent.left; leftMargin: 100; right: parent.right
                              verticalCenter: parent.verticalCenter }
                    height: 24
                    contentWidth: chips.width
                    flickableDirection: Flickable.HorizontalFlick
                    clip: true

                    Row {
                        id: chips
                        spacing: 4

                        Repeater {
                            model: Remotes.hosts

                            SettingsChip {
                                required property var modelData
                                // A profile we saved is marked; the rest are
                                // ssh's own, offered but not yet ours.
                                text: modelData.saved ? "● " + modelData.id : modelData.id
                                onClicked: Remotes.pick_host(modelData.id)
                            }
                        }

                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            visible: Remotes.hosts.length === 0
                            text: qsTr("none saved, and none in ~/.ssh/config")
                            color: Theme.chat_colors.dim
                            font.family: Theme.sans_family
                            font.pixelSize: 11
                        }
                    }
                }
            }

            RemoteField { id: nameField; label: qsTr("Name")
                          placeholderText: qsTr("A label for this host, e.g. gpu-box")
                          text: Remotes.form_name }
            RemoteField { id: hostField; label: qsTr("Hostname")
                          placeholderText: qsTr("hostname or IP (or an ssh_config alias)")
                          text: Remotes.form_hostname }
            RemoteField { id: userField; label: qsTr("User")
                          placeholderText: qsTr("optional — defaults to your login name")
                          text: Remotes.form_user }

            Item {
                width: parent.width
                height: 26

                Text {
                    anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                    width: 96
                    text: qsTr("Port")
                    color: Theme.chat_colors.dim
                    font.family: Theme.sans_family
                    font.pixelSize: 12
                }

                SettingsStepper {
                    id: portField
                    anchors { left: parent.left; leftMargin: 100; verticalCenter: parent.verticalCenter }
                    value: Remotes.form_port
                    from: 1
                    to: 65535
                    onChanged: (port) => value = port
                }
            }

            RemoteField { id: identityField; label: qsTr("Identity file")
                          placeholderText: qsTr("optional — defaults to your ssh keys")
                          text: Remotes.form_identity
                          browsable: true
                          onBrowse: identityDialog.open() }
            RemoteField { id: pathField; label: qsTr("Remote path")
                          placeholderText: qsTr("/absolute/path/to/project on the remote")
                          text: Remotes.form_path }

            Text {
                width: parent.width
                visible: text !== ""
                text: dialog.error !== "" ? dialog.error : Remotes.status
                color: dialog.error !== "" ? Theme.chat_colors.error
                                           : Theme.chat_colors.dim
                font.family: Theme.sans_family
                font.pixelSize: 11
                wrapMode: Text.Wrap
                topPadding: 2
            }

            Item {
                width: parent.width
                height: 30

                SettingsChip {
                    anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                    text: Remotes.probing ? qsTr("Testing…") : qsTr("Test Connection")
                    enabled: !Remotes.probing
                    onClicked: {
                        dialog.error = ""
                        Remotes.test_connection(nameField.text, hostField.text,
                                                userField.text, portField.value,
                                                identityField.text)
                    }
                }

                Row {
                    anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                    spacing: 6
                    SettingsChip { text: qsTr("Cancel"); onClicked: Remotes.hide() }
                    SettingsChip {
                        text: Remotes.editing ? qsTr("Save") : qsTr("Add")
                        onClicked: dialog.error = Remotes.save(
                            nameField.text, hostField.text, userField.text,
                            portField.value, identityField.text, pathField.text)
                    }
                }
            }
        }
    }

    property string error: ""

    FileDialog {
        id: identityDialog
        title: qsTr("Identity file")
        onAccepted: identityField.text = selectedFile.toString().replace("file://", "")
    }

    focus: visible
    Keys.onEscapePressed: Remotes.hide()
    onVisibleChanged: {
        if (visible) {
            error = ""
            forceActiveFocus()
        }
    }
}
