import QtQuick
import "../common"

// One panel in the dock: its header, and whatever it holds.
//
// Panels meet each other and the window's edges directly; the only line between
// two of them is the divider the dock paints, so there is no margin here — one
// would open a gap of window colour beside it.
Item {
    id: panel

    property string key
    property string title
    // An anchored panel has no grip: History and the conversation are fixed
    // where they are, and a drag on them could only fail.
    property bool draggable: true
    property alias headerTrailing: header.trailing
    // The adopted panel's own tab strip and controls, if it has them; see
    // PanelHeader.
    property alias headerTabs: header.tabs
    property alias headerTools: header.tools
    property bool dragging: false
    default property alias content: body.data
    // Where an adopted panel goes: under the header, not over it.
    readonly property alias contentArea: body

    signal dragStarted(point globalPos)
    signal dragMoved(point globalPos)
    signal dragEnded(point globalPos)

    PanelHeader {
        id: header
        anchors { left: parent.left; right: parent.right; top: parent.top }
        title: panel.title
        dragging: panel.dragging
        visible: panel.draggable
        height: panel.draggable ? 32 : 0
        onDragStarted: function (pos) { panel.dragStarted(pos) }
        onDragMoved: function (pos) { panel.dragMoved(pos) }
        onDragEnded: function (pos) { panel.dragEnded(pos) }
    }

    Item {
        id: body
        anchors {
            left: parent.left; right: parent.right; bottom: parent.bottom
            top: header.visible ? header.bottom : parent.top
        }
    }
}
