import QtQuick
import QtQuick.Controls
import "../common"

// One page of the settings window: a scrolling column of sections and rows.
//
// The page owns the rhythm — the width of the column, the space above a
// section, the hairline under it — so a page's own file is a list of what it
// sets rather than a layout.
Flickable {
    id: page

    default property alias body: column.data

    contentWidth: width
    contentHeight: column.implicitHeight + 24
    boundsBehavior: Flickable.StopAtBounds
    clip: true

    ScrollBar.vertical: ThinScrollBar {}

    Column {
        id: column
        x: 20
        y: 4
        width: page.width - 40 - 12
        spacing: 0
    }
}
