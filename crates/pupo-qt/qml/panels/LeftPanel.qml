import QtQuick
import QtQuick.Controls
import "../common"

// The History pane: the Pi sessions recorded for this workspace folder, plus
// whatever is running right now.
//
// The one panel that is not a card. It paints its own background out to its
// edges, so it reads as part of the window rather than as something resting on
// it; the hairline separating it from the conversation is the dock's divider,
// the same one between any two panels, and a border here would double it.
Rectangle {
    id: panel
    color: Theme.colors.sidebar

    // History rows use the proportional face: a session's title is prose, and a
    // mono grid makes a list of sentences read like a table of data.
    readonly property color rowText: Theme.name === "dark" ? "#c8cad0" : "#383a42"
    readonly property color rowSubtitle: Theme.name === "dark" ? "#7a7d85" : "#8e8b86"

    Column {
        anchors.fill: parent
        anchors.margins: 12
        spacing: 10

        Rectangle {
            id: newButton
            width: parent.width
            height: 30
            radius: 6
            color: newMouse.containsMouse ? Theme.colors.hover : Theme.colors.header
            border.width: 1
            border.color: Theme.colors.card_border

            Text {
                anchors.centerIn: parent
                text: "＋  " + qsTr("New Session")
                color: newMouse.containsMouse ? Theme.colors.text : panel.rowSubtitle
                font.family: Theme.sans_family
                font.pixelSize: 14
                font.weight: Font.Normal
            }

            MouseArea {
                id: newMouse
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: History.request_new_session()
            }
        }

        // The pinned sessions, above the rest and in their own list. Both
        // headings hide when their list is empty, so a pane with nothing pinned
        // looks exactly as it did before anything was.
        Text {
            id: pinnedLabel
            visible: pinnedList.count > 0
            text: qsTr("Pinned")
            color: panel.rowSubtitle
            font.family: Theme.sans_family
            font.pixelSize: 14
        }

        ListView {
            id: pinnedList
            width: parent.width
            visible: count > 0
            // Sized to its contents so the pinned group takes only the room it
            // needs, but never more than a third of the pane: pinning a dozen
            // sessions must not push the recent ones off the bottom.
            height: Math.min(contentHeight, panel.height / 3)
            clip: true
            spacing: 2
            model: History.pinned
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}

            delegate: SessionRow {
                width: pinnedList.width
                titleColor: panel.rowText
                subtitleColor: panel.rowSubtitle
            }
        }

        // Nothing recorded yet: the list says so itself, so a heading over an
        // empty column would only repeat it.
        Text {
            id: recentsLabel
            visible: list.count > 0
            text: qsTr("Recents")
            color: panel.rowSubtitle
            font.family: Theme.sans_family
            font.pixelSize: 14
        }

        ListView {
            id: list
            width: parent.width
            // Whatever the button, the headings and the pinned list leave. A
            // hidden child takes no room in the Column, and its gap goes with
            // it, so each one is only subtracted when it is actually shown.
            height: parent.height - newButton.height - parent.spacing
                    - (pinnedLabel.visible ? pinnedLabel.height + parent.spacing : 0)
                    - (pinnedList.visible ? pinnedList.height + parent.spacing : 0)
                    - (recentsLabel.visible ? recentsLabel.height + parent.spacing : 0)
            clip: true
            spacing: 2
            model: History.sessions
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}

            // Nothing recorded and nothing running: say so rather than leaving
            // a blank column that reads as a panel that failed to load.
            Text {
                anchors.centerIn: parent
                visible: list.count === 0 && pinnedList.count === 0
                text: qsTr("No previous sessions")
                color: panel.rowSubtitle
                font.family: Theme.sans_family
                font.pixelSize: 13
            }

            delegate: SessionRow {
                width: list.width
                titleColor: panel.rowText
                subtitleColor: panel.rowSubtitle
            }
        }
    }
}
