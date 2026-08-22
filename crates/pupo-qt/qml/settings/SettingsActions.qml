import QtQuick

// The buttons under a group, right-aligned under the control column.
//
// Wrapped in an Item because a Column positions its children and ignores their
// anchors: a Row that tried to anchor itself right would simply be laid out
// left, silently.
Item {
    id: actions

    default property alias buttons: row.data

    width: parent ? parent.width : 0
    implicitHeight: row.implicitHeight + 6

    Row {
        id: row
        anchors { right: parent.right; top: parent.top; topMargin: 2 }
        spacing: 6
    }
}
