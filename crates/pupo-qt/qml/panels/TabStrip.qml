import QtQuick
import "../common"

// The tab strip a panel wears in its header — terminals now, browser pages
// later.
//
// One component for both, because a tab is a tab: the same height, the same
// rounded current tab, the same close affordance that only appears where it can
// be used. Two strips written separately are two strips that drift.
//
// It mounts into the dock's own panel header rather than sitting under it (see
// DockPanel.qml), so a panel with tabs has one strip across its top instead of a
// title bar with a second bar beneath it.
Item {
    id: strip

    // `{ id, title, bell, closed }` per tab, left to right.
    property var tabs: []
    // The current tab's id, or -1.
    property int current: -1

    signal selected(int id)
    signal closed(int id)
    signal added()
    signal moved(int from, int to)

    implicitHeight: 32

    Row {
        id: row
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        anchors.bottomMargin: 1
        spacing: 2

        Repeater {
            id: repeater
            model: strip.tabs

            Rectangle {
                id: tab
                required property var modelData
                required property int index

                readonly property bool isCurrent: modelData.id === strip.current

                // Wide enough to read a shell's title, narrow enough that four
                // tabs still fit in a docked column.
                width: Math.min(160, Math.max(72, label.implicitWidth + 34))
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
                    visible: tab.modelData.bell && !tab.isCurrent
                    color: "#e0a030"
                }

                Text {
                    id: label
                    anchors {
                        left: bell.visible ? bell.right : parent.left
                        leftMargin: bell.visible ? 5 : 9
                        right: close.left
                        rightMargin: 2
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
                    font.italic: tab.modelData.closed
                    elide: Text.ElideRight
                }

                // Only on the tab you are pointing at or the one you are in: a
                // close button on every tab is a row of buttons, and the one you
                // want is no easier to find.
                IconButton {
                    id: close
                    anchors { right: parent.right; rightMargin: 3; verticalCenter: parent.verticalCenter }
                    implicitWidth: 16
                    implicitHeight: 16
                    glyphSize: 10
                    radius: 4
                    glyph: "x"
                    visible: tab.isCurrent || tabMouse.containsMouse
                    onClicked: strip.closed(tab.modelData.id)
                }

                MouseArea {
                    id: tabMouse
                    anchors.fill: parent
                    anchors.rightMargin: close.visible ? close.width + 4 : 0
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onPressed: strip.selected(tab.modelData.id)

                    // Reorder by dragging past a neighbour's middle. The tab
                    // itself never moves under the pointer — the list is
                    // rebuilt from the model, and the pointer is already over
                    // the tab's new place by the time it is.
                    onPositionChanged: function (mouse) {
                        if (!pressed)
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
