import QtQuick
import "../common"

// The strip every panel wears along its top, whatever it puts in it.
//
// The strip is a surface a shade off the panel below it, closed with the
// hairline a card border would have drawn, so a panel reads as content under a
// header rather than as one flat sheet. It runs the panel's full width — the
// card's padding starts underneath it. The grip in it is what a drag grabs.
Rectangle {
    id: header

    property string title
    property alias trailing: trailingArea.data
    // A panel that has tabs of its own puts them here instead of a title. One
    // strip across a panel's top reads as one panel; a title bar with a second
    // bar of tabs beneath it reads as two.
    property Item tabs
    onTabsChanged: {
        if (!tabs)
            return
        tabs.parent = tabArea
        tabs.anchors.fill = tabArea
    }
    // The panel's own controls — a Refresh button, say — at the strip's right
    // end. Handed up the same way the tabs are, so a panel never grows a second
    // toolbar of its own under the one it already has.
    property Item tools
    onToolsChanged: {
        if (tools)
            tools.parent = trailingArea
    }
    // Set while this panel is the one being dragged, so the grip stays lit even
    // once the pointer has left it.
    property bool dragging: false

    signal dragStarted(point globalPos)
    signal dragMoved(point globalPos)
    signal dragEnded(point globalPos)

    height: 32
    color: Theme.colors.header

    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 1
        color: Theme.colors.card_border
    }

    // The grip sits inside the strip, so it paints transparent over it and
    // carries nothing but the glyph itself.
    Item {
        id: grip
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        anchors.bottomMargin: 1
        width: 26

        Rectangle {
            anchors.fill: parent
            anchors.margins: 3
            radius: 4
            color: (gripMouse.containsMouse || header.dragging)
                   ? Theme.colors.hover : "transparent"
        }

        // Six dots: the one glyph that reads as "grab me" at this size without
        // needing a word for it.
        Grid {
            anchors.centerIn: parent
            columns: 2
            rowSpacing: 3
            columnSpacing: 3
            Repeater {
                model: 6
                delegate: Rectangle {
                    width: 2; height: 2; radius: 1
                    color: Theme.name === "dark" ? "#5a5d65" : "#b0aeaa"
                }
            }
        }

        MouseArea {
            id: gripMouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.OpenHandCursor
            onPressed: function (mouse) {
                header.dragStarted(mapToGlobal(mouse.x, mouse.y))
            }
            onPositionChanged: function (mouse) {
                if (pressed)
                    header.dragMoved(mapToGlobal(mouse.x, mouse.y))
            }
            onReleased: function (mouse) {
                header.dragEnded(mapToGlobal(mouse.x, mouse.y))
            }
        }
    }

    Item {
        id: tabArea
        anchors {
            left: grip.right
            right: trailingArea.left
            top: parent.top
            bottom: parent.bottom
        }
        anchors.bottomMargin: 1
    }

    Row {
        id: trailingArea
        anchors { right: parent.right; top: parent.top; bottom: parent.bottom }
        anchors.rightMargin: 4
        anchors.bottomMargin: 1
        spacing: 2
    }

    Text {
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: grip.right
        anchors.right: trailingArea.left
        anchors.rightMargin: 6
        visible: !header.tabs
        text: header.title
        color: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
        font.family: Theme.sans_family
        font.pixelSize: 11
        font.weight: Font.DemiBold
        elide: Text.ElideRight
    }
}
