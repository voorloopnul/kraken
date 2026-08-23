import QtQuick
import "../common"

// The tab strip a panel wears in its header — terminals, browser pages, and the
// Git pane's two views of the repository.
//
// One component for all of them, because a tab is a tab: the same height, the
// same rounded current tab, the same place to close one. Strips written
// separately are strips that drift.
//
// It mounts into the dock's own panel header rather than sitting under it (see
// DockPanel.qml), so a panel with tabs has one strip across its top instead of a
// title bar with a second bar beneath it.
//
// Closing is deliberately not a button inside the tab. A tab is a target you
// aim at to *switch* to it, and a per-tab close button puts a destructive
// control inside that target — on a numbered terminal tab it was half of it, so
// half of every click at a shell was a click that killed one. Instead the
// closing lives at the far end of the strip and acts on the tab you are already
// looking at: you cannot destroy a shell you have not read, and no aim at a tab
// can miss into it. Middle-click closes a tab outright for anyone who wants the
// short way; it is not a gesture that happens by accident.
Item {
    id: strip

    // `{ id, title, bell, closed }` per tab, left to right.
    property var tabs: []
    // The current tab's id, or -1.
    property int current: -1
    // Tabs that are the panel's own views rather than things anyone opened:
    // nothing to close, nothing to add, and no order to put them in. What is
    // left is the one thing they are for, which is choosing between them.
    property bool fixed: false

    signal selected(int id)
    signal closed(int id)
    signal added()
    signal moved(int from, int to)

    implicitHeight: 32

    Row {
        id: row
        anchors {
            left: parent.left; top: parent.top; bottom: parent.bottom
            // Never under the close button: a tab hidden behind it is a tab
            // whose click lands on closing something else.
            right: closeCurrent.visible ? closeCurrent.left : parent.right
        }
        anchors.bottomMargin: 1
        clip: true
        spacing: 2

        Repeater {
            id: repeater
            model: strip.tabs

            Rectangle {
                id: tab
                required property var modelData
                required property int index

                readonly property bool isCurrent: modelData.id === strip.current

                // The label and its padding, and nothing else: with no button
                // sharing the tab, a terminal's bare number gets a tab the size
                // of a number. The cap is there so one long page title cannot
                // push the rest of the strip off the panel.
                width: Math.min(160, label.implicitWidth + 18)
                height: strip.height - 1
                radius: 5
                color: tab.isCurrent ? Theme.colors.card
                     : tabMouse.containsMouse ? Theme.colors.hover
                     : "transparent"

                // Rung while you were reading another tab. The dot is the whole
                // point of a bell nobody heard.
                Rectangle {
                    id: bell
                    anchors { left: parent.left; leftMargin: 8; verticalCenter: parent.verticalCenter }
                    width: 5; height: 5; radius: 3
                    // Coerced, because a fixed tab carries a title and an id
                    // and nothing else: a tab that cannot ring has no `bell`
                    // for a bool property to read.
                    visible: !!tab.modelData.bell && !tab.isCurrent
                    color: "#e0a030"
                }

                Text {
                    id: label
                    anchors {
                        left: bell.visible ? bell.right : parent.left
                        leftMargin: bell.visible ? 5 : 9
                        right: parent.right
                        rightMargin: 9
                        verticalCenter: parent.verticalCenter
                    }
                    text: tab.modelData.title
                    // A shell that has exited keeps its tab so its last screen
                    // can be read, and says so rather than looking live.
                    color: tab.modelData.closed ? Theme.chat_colors.dim
                         : tab.isCurrent ? Theme.colors.text
                         : (Theme.name === "dark" ? "#9a9da5" : "#5a5d65")
                    font.family: Theme.sans_family
                    font.pixelSize: 11
                    font.italic: !!tab.modelData.closed
                    elide: Text.ElideRight
                }

                // The whole tab selects — there is nothing else in it to hit.
                MouseArea {
                    id: tabMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.MiddleButton
                    onPressed: function (mouse) {
                        if (mouse.button === Qt.MiddleButton) {
                            if (!strip.fixed)
                                strip.closed(tab.modelData.id)
                            return
                        }
                        strip.selected(tab.modelData.id)
                    }

                    // Reorder by dragging past a neighbour's middle. The tab
                    // itself never moves under the pointer — the list is
                    // rebuilt from the model, and the pointer is already over
                    // the tab's new place by the time it is.
                    onPositionChanged: function (mouse) {
                        if (!pressed || strip.fixed)
                            return
                        const at = mapToItem(row, mouse.x, 0).x
                        const over = Math.floor(at / (tab.width + row.spacing))
                        if (over !== tab.index && over >= 0 && over < repeater.count)
                            strip.moved(tab.index, over)
                    }
                }
            }
        }

        // New tab. Inside the Row so it follows the last tab rather than
        // sitting at a fixed place the tabs grow past.
        IconButton {
            visible: !strip.fixed
            anchors.verticalCenter: parent.verticalCenter
            implicitWidth: 22
            implicitHeight: 22
            glyphSize: 12
            glyph: "plus"
            tooltip: qsTr("New tab")
            onClicked: strip.added()
        }
    }

    // Close the tab you are looking at, pinned to the strip's far end — the
    // width of the whole strip away from the tabs, so nothing aimed at one can
    // land on it.
    //
    // A minus rather than a cross: it is the answer to the plus beside the
    // tabs, one fewer where that one is one more, and a cross at the right end
    // of a panel's header would read as closing the panel.
    IconButton {
        id: closeCurrent
        visible: !strip.fixed && strip.current >= 0
        anchors { right: parent.right; verticalCenter: parent.verticalCenter }
        implicitWidth: 22
        implicitHeight: 22
        glyphSize: 12
        glyph: "minus"
        tooltip: qsTr("Close tab")
        onClicked: strip.closed(strip.current)
    }
}
