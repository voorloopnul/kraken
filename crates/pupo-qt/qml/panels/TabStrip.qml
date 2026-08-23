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
// Closing is a second gesture on the tab itself, never a button sitting in it.
// A tab is a target you aim at to *switch* to it, and a close button inside that
// target means half of every click at a shell is a click that kills one — which
// is what a numbered terminal tab used to be. A button at the far end of the
// strip missed the other way: a minus in a panel header reads as minimising the
// panel, not as closing what is in it.
//
// So the tab arms itself. Right-click one and its label turns into a cross: the
// tab you aimed at is now the close button, and the left-click that follows
// closes it. Nothing is destroyed by a single click, the control appears where
// the pointer already is, and moving off the tab or right-clicking it again puts
// the label back. Middle-click still closes outright for anyone who wants the
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

    // The tab showing its close cross instead of its label, or -1. One at a
    // time: arming another disarms this one, so there is never a strip of
    // tabs all offering to be destroyed.
    //
    // Held as an id rather than an index, and ids are never reused, so a tab
    // that goes away while armed leaves this pointing at nothing rather than at
    // whatever took its place. It survives the model being rebuilt on the way
    // past — a bell in another shell rebuilds `tabs`, and disarming on that
    // would take the cross away between the right-click and the left one.
    property int armed: -1

    signal selected(int id)
    signal closed(int id)
    signal added()
    signal moved(int from, int to)

    implicitHeight: 32

    Row {
        id: row
        anchors {
            left: parent.left; top: parent.top; bottom: parent.bottom
            right: parent.right
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
                // Armed: the label has stepped aside for the cross, and the
                // next left-click on this tab closes it. Only ever a tab
                // anybody opened — a panel's own views have nothing to close.
                readonly property bool armed: !strip.fixed
                                              && modelData.id === strip.armed

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
                    // Kept in place rather than dropped, so the tab keeps the
                    // width its title asks for: a tab that narrowed as it armed
                    // would move its neighbours out from under the pointer.
                    visible: !tab.armed
                }

                // The armed tab's cross, in the label's place. A vendored glyph
                // rather than a character, and red, because the click that
                // follows it destroys something.
                Image {
                    anchors.centerIn: parent
                    width: 11
                    height: 11
                    sourceSize: Qt.size(22, 22)
                    smooth: true
                    visible: tab.armed
                    source: Theme.icon("x", Theme.chat_colors.error)
                }

                // The whole tab selects, or closes once it is armed — there is
                // nothing else in it to hit either way.
                MouseArea {
                    id: tabMouse
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton
                    // The label comes back when the pointer leaves: an armed
                    // tab is a state you are standing in, not one the strip
                    // keeps for you to forget about and click into later.
                    onExited: if (tab.armed) strip.armed = -1
                    onPressed: function (mouse) {
                        if (mouse.button === Qt.RightButton) {
                            if (!strip.fixed)
                                strip.armed = tab.armed ? -1 : tab.modelData.id
                            return
                        }
                        if (mouse.button === Qt.MiddleButton) {
                            if (!strip.fixed)
                                strip.closed(tab.modelData.id)
                            return
                        }
                        if (tab.armed) {
                            strip.closed(tab.modelData.id)
                            strip.armed = -1
                            return
                        }
                        strip.selected(tab.modelData.id)
                    }

                    // Reorder by dragging past a neighbour's middle. The tab
                    // itself never moves under the pointer — the list is
                    // rebuilt from the model, and the pointer is already over
                    // the tab's new place by the time it is.
                    onPositionChanged: function (mouse) {
                        if (!pressed || strip.fixed || tab.armed)
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
}
