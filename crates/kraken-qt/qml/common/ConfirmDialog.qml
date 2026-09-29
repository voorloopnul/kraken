import QtQuick
import QtQuick.Controls
import "../settings"

// A question, or a notice, over the whole window: a titled card on a dimmed
// app, with the app's own buttons rather than the platform's Yes/No.
//
// The buttons name what they do — "Delete", not "Yes" — so the answer can be
// given without re-reading the question. A destructive one is drawn in the
// danger colour, and for that one Enter does nothing: only a click deletes, so
// a keystroke meant for the composer cannot. Escape and a click outside are a
// cancel, the one answer safe to give by accident.
//
// With `alert` set it is a notice instead: one button, and nothing to decide.
Popup {
    id: dialog

    property string title
    property string message
    property string confirmText: qsTr("OK")
    property string cancelText: qsTr("Cancel")
    property bool destructive: false
    property bool alert: false
    /// For a message that is some tool's own output — git's refusal, say —
    /// which reads as it was written only in the mono face.
    property bool monoMessage: false

    signal confirmed()

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(parent ? parent.width - 80 : 400, 400)
    modal: true
    focus: true
    padding: 20
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    Overlay.modal: Rectangle {
        color: Theme.name === "dark" ? Qt.rgba(0, 0, 0, 0.45)
                                     : Qt.rgba(0, 0, 0, 0.28)
    }

    background: Rectangle {
        radius: 10
        color: Theme.colors.card
        border.width: 1
        border.color: Theme.colors.card_border
    }

    function accept() {
        close()
        confirmed()
    }

    contentItem: Column {
        spacing: 8

        Keys.onReturnPressed: if (!dialog.destructive) dialog.accept()
        Keys.onEnterPressed: if (!dialog.destructive) dialog.accept()

        Text {
            width: parent.width
            text: dialog.title
            visible: text !== ""
            color: Theme.colors.text
            font.family: Theme.sans_family
            font.pixelSize: 13
            font.weight: Font.DemiBold
            wrapMode: Text.Wrap
        }

        Text {
            width: parent.width
            text: dialog.message
            visible: text !== ""
            color: dialog.monoMessage ? Theme.colors.text : Theme.chat_colors.dim
            font.family: dialog.monoMessage ? Theme.mono_family : Theme.sans_family
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }

        Item { width: 1; height: 8 }

        Row {
            anchors.right: parent.right
            spacing: 6

            SettingsChip {
                text: dialog.cancelText
                visible: !dialog.alert
                onClicked: dialog.close()
            }
            SettingsChip {
                text: dialog.confirmText
                danger: dialog.destructive
                onClicked: dialog.accept()
            }
        }
    }

    onOpened: contentItem.forceActiveFocus()
}
