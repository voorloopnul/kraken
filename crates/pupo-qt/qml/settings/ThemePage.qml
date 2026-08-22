import QtQuick
import "../common"

// Appearance: the palette, and the two font scales.
//
// Every control here writes straight to the Theme object, which is where the
// three of them already live and which persists them itself — a page holding a
// copy would be a second answer to a question that has one.
SettingsPage {
    SettingsSection { text: qsTr("Appearance"); first: true }

    SettingRow {
        title: qsTr("Theme")
        description: qsTr("Colours for the whole app: chrome, panels, and the "
                          + "terminal palette. Applies immediately, to every "
                          + "open workspace.")

        Row {
            spacing: 6

            Repeater {
                model: ["light", "dark"]

                Rectangle {
                    required property string modelData
                    readonly property bool current: Theme.name === modelData
                    width: 78
                    height: 26
                    radius: 5
                    color: current ? Theme.colors.accent_soft
                         : swatchMouse.containsMouse ? Theme.colors.hover
                         : Theme.colors.header
                    border.width: 1
                    border.color: current ? Theme.colors.accent_text
                                          : Theme.colors.card_border

                    Text {
                        anchors.centerIn: parent
                        text: parent.modelData === "dark" ? qsTr("Dark") : qsTr("Light")
                        color: parent.current ? Theme.colors.accent_text
                                              : Theme.colors.text
                        font.family: Theme.sans_family
                        font.pixelSize: 12
                    }

                    MouseArea {
                        id: swatchMouse
                        anchors.fill: parent
                        hoverEnabled: true
                        cursorShape: Qt.PointingHandCursor
                        onClicked: Theme.name = parent.modelData
                    }
                }
            }
        }
    }

    SettingRow {
        title: qsTr("Conversation Font Size")
        description: qsTr("Base size for the chat. Message text is set at it, and "
                          + "the rest of the pane — headings, code, the composer, "
                          + "tool detail — is sized from it. Applies to open "
                          + "transcripts as well as new ones.")

        SettingsStepper {
            value: Theme.chat_font_size
            from: Theme.chat_font_min
            to: Theme.chat_font_max
            suffix: " px"
            onChanged: (size) => Theme.chat_font_size = size
        }
    }

    SettingRow {
        title: qsTr("Terminal Font Size")
        description: qsTr("Point size for terminal text. Applies immediately to "
                          + "every open terminal and to new terminal tabs.")

        SettingsStepper {
            value: Theme.terminal_font_size
            from: Theme.terminal_font_min
            to: Theme.terminal_font_max
            suffix: " pt"
            onChanged: (size) => Theme.terminal_font_size = size
        }
    }
}
