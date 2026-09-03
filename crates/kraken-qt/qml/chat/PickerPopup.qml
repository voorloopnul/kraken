import QtQuick
import QtQuick.Controls
import "../common"

// The composer's two menus: the model list and the reasoning-effort list.
//
// One component for both, because the difference between them is a filter
// field. The model list can run to hundreds of entries across every configured
// provider, which is unusable without a search; the effort list is four rows
// and a search box on it would be furniture.
Popup {
    id: picker

    // `{ label, detail, current, payload }` per row. `payload` is whatever the
    // caller wants back — a level name, or a `{provider, id}` pair.
    property var entries: []
    property bool searchable: false
    property string emptyText: qsTr("Nothing to choose from")

    signal picked(var payload)

    padding: 6
    modal: false
    focus: true
    closePolicy: Popup.CloseOnEscape | Popup.CloseOnPressOutside

    // Tall enough to be worth scrolling, short enough to stay on screen next to
    // a composer that already sits near the bottom of the window.
    implicitWidth: 320
    implicitHeight: Math.min(360, column.implicitHeight + 12)

    background: Rectangle {
        color: Theme.colors.header
        border.width: 1
        border.color: Theme.colors.card_border
        radius: 8
    }

    onOpened: {
        filter.text = ""
        if (searchable)
            filter.forceActiveFocus()
    }

    // The rows left after the filter. Matching on the label and the detail
    // together is what lets "anthropic opus" and "opus" both find the same row.
    readonly property var shown: {
        const needle = filter.text.trim().toLowerCase()
        if (needle === "")
            return entries
        return entries.filter(function (entry) {
            return (entry.label + " " + entry.detail).toLowerCase().indexOf(needle) >= 0
        })
    }

    Column {
        id: column
        width: parent.width
        spacing: 6

        TextField {
            id: filter
            width: parent.width
            visible: picker.searchable
            height: visible ? implicitHeight : 0
            placeholderText: qsTr("Search")
            color: Theme.colors.text
            font.family: Theme.mono_family
            font.pixelSize: Theme.secondary_font_size
            background: Rectangle {
                color: Theme.colors.card
                border.width: 1
                border.color: Theme.colors.card_border
                radius: 4
            }
            onAccepted: {
                if (picker.shown.length > 0)
                    picker.picked(picker.shown[0].payload)
            }
        }

        Text {
            width: parent.width
            visible: picker.shown.length === 0
            text: picker.emptyText
            color: Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: Theme.secondary_font_size
            wrapMode: Text.Wrap
        }

        ListView {
            id: list
            width: parent.width
            height: Math.min(300, contentHeight)
            clip: true
            model: picker.shown
            boundsBehavior: Flickable.StopAtBounds
            spacing: 1

            ScrollBar.vertical: ThinScrollBar {}

            delegate: Rectangle {
                id: row
                required property var modelData
                width: list.width
                height: rowLabel.implicitHeight + 10
                radius: 4
                color: rowMouse.containsMouse ? Theme.colors.hover
                     : modelData.current ? Theme.colors.accent_soft
                     : "transparent"

                Text {
                    id: rowLabel
                    anchors {
                        left: parent.left; right: parent.right
                        verticalCenter: parent.verticalCenter
                        leftMargin: 8; rightMargin: 8
                    }
                    text: row.modelData.detail === ""
                          ? row.modelData.label
                          : row.modelData.label + "  ·  " + row.modelData.detail
                    color: row.modelData.current ? Theme.colors.accent_text
                                                 : Theme.colors.text
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    elide: Text.ElideRight
                }

                MouseArea {
                    id: rowMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: picker.picked(row.modelData.payload)
                }
            }
        }
    }
}
