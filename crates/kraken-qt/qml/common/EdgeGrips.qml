import QtQuick

// Invisible strips overlaid along the window's edges.
//
// A custom title bar removes the native frame, and with it native resizing.
// Pressing a strip hands the drag back to the window manager; the ends of each
// strip double as corner grips by adding the perpendicular edge.
Item {
    id: grips

    property Window target
    // Grabbing within this many pixels of an edge starts a resize.
    property int margin: 6
    // Within this many pixels of a strip's end the grab is a corner resize.
    readonly property int cornerMargin: 14

    anchors.fill: parent

    Repeater {
        model: [
            { edge: Qt.LeftEdge,   horizontal: false },
            { edge: Qt.RightEdge,  horizontal: false },
            { edge: Qt.TopEdge,    horizontal: true },
            { edge: Qt.BottomEdge, horizontal: true }
        ]
        delegate: MouseArea {
            required property var modelData
            readonly property bool horizontal: modelData.horizontal
            readonly property int edge: modelData.edge

            width: horizontal ? grips.width : grips.margin
            height: horizontal ? grips.margin : grips.height
            x: edge === Qt.RightEdge ? grips.width - grips.margin : 0
            y: edge === Qt.BottomEdge ? grips.height - grips.margin : 0

            hoverEnabled: true
            cursorShape: cursorFor(edgesAt(mouseX, mouseY))

            function edgesAt(mx, my) {
                let edges = edge
                const along = horizontal ? mx : my
                const length = horizontal ? width : height
                if (along <= grips.cornerMargin)
                    edges |= horizontal ? Qt.LeftEdge : Qt.TopEdge
                else if (along >= length - grips.cornerMargin)
                    edges |= horizontal ? Qt.RightEdge : Qt.BottomEdge
                return edges
            }

            function cursorFor(edges) {
                const left = (edges & Qt.LeftEdge) !== 0
                const right = (edges & Qt.RightEdge) !== 0
                const top = (edges & Qt.TopEdge) !== 0
                const bottom = (edges & Qt.BottomEdge) !== 0
                if ((top && left) || (bottom && right))
                    return Qt.SizeFDiagCursor
                if ((top && right) || (bottom && left))
                    return Qt.SizeBDiagCursor
                return (left || right) ? Qt.SizeHorCursor : Qt.SizeVerCursor
            }

            onPressed: function (event) {
                if (grips.target)
                    grips.target.startSystemResize(edgesAt(event.x, event.y))
            }
        }
    }
}
