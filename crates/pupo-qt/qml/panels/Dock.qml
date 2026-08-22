import QtQuick
import QtQuick.Controls
import "../common"

// The columns, the dividers between them, and the drop indicator over the lot.
//
// The arrangement is `Dock`'s (in Rust); this lays it out and reports back
// where everything landed. Panels are built once and kept: a panel that was
// destroyed and rebuilt on every re-dock would lose its terminals, its browser
// and its scroll position, which is the whole reason a drag is worth having.
Item {
    id: dock

    // key -> the Item to show in that panel's slot.
    property var panels: ({})
    property var titles: ({})
    // Keys that carry no grip: History and the conversation are fixed anchors.
    property var anchored: ["left", "center"]

    // The divider between two panels is the border they no longer draw
    // themselves, so it is the same colour a card's border would have been. The
    // handle carrying it is wider than the line, and fills the rest with the
    // surfaces on either side, so the two panels read as one surface split by a
    // border rather than as two with a gutter between them.
    readonly property int dividerWidth: 5

    // The width each side column actually gets, by column index.
    //
    // Open enough panels and what they ask for comes to more than the window
    // has, and the conversation cannot pay for it alone — it has a floor of its
    // own. `fit_columns` shares the shortfall out, and it is the one that knows
    // a column pinned at its floor cannot take its full share of the cut.
    property var fitted: ({})
    // The side columns added up, once fitted.
    property int squeezedTotal: 0

    // Recomputed rather than bound: the terms are `userWidth` on sibling
    // delegates a Repeater may not have built yet, and a binding over `itemAt`
    // would re-evaluate to a half-filled sum while it is still building.
    function recomputeFixed() {
        // The whole width, with nothing set aside for the dividers: each one is
        // drawn *inside* the column it belongs to, anchored over its right edge,
        // so it takes no room in the row. Subtracting it here as well left a
        // strip of empty dock beyond the last column, five pixels per divider.
        const available = dock.width

        const spec = []
        const at = []
        for (let i = 0; i < columns.count; i++) {
            const item = columns.itemAt(i)
            if (!item || item.isStretch)
                continue
            spec.push({ width: item.userWidth,
                        min: DockModel.min_width(item.modelData[0]) })
            at.push(i)
        }

        const widths = JSON.parse(DockModel.fit_columns(available, JSON.stringify(spec)))
        const next = {}
        let got = 0
        for (let k = 0; k < at.length; k++) {
            next[at[k]] = widths[k]
            got += widths[k]
        }
        fitted = next
        squeezedTotal = got
    }

    // What the conversation is left with once the others have taken theirs.
    // Nothing is reserved for dividers here either, for the same reason.
    readonly property int stretchWidth:
        Math.max(dock.width - squeezedTotal, DockModel.min_width("center"))

    // What a panel paints itself. History is the one that is a surface rather
    // than something resting on one, and the divider beside it has to agree.
    function surfaceOf(key) {
        return key === "left" ? Theme.colors.sidebar : Theme.colors.card
    }

    function reportGeometry() {
        const rects = []
        for (let i = 0; i < columnRow.children.length; i++) {
            const column = columnRow.children[i]
            if (!column || column.objectName !== "dockColumn")
                continue
            const at = column.mapToItem(dock, 0, 0)
            rects.push({ x: at.x, y: at.y, width: column.width, height: column.height })
        }
        DockModel.set_geometry(JSON.stringify(rects))
    }

    // Turn the squeeze into widths the columns own outright.
    //
    // Called when a drag starts: from then on the columns are the size the user
    // put them at, and the proportional pass has nothing left to do until the
    // window narrows or another panel opens.
    //
    // Every painted width is read before any is written — assigning one
    // recomputes the squeeze, which would move the ones not yet read.
    function bakeWidths() {
        const painted = []
        for (let i = 0; i < columns.count; i++) {
            const item = columns.itemAt(i)
            painted.push(item && !item.isStretch ? item.paintedWidth : -1)
        }
        for (let i = 0; i < columns.count; i++) {
            const item = columns.itemAt(i)
            if (item && painted[i] >= 0)
                item.userWidth = painted[i]
        }
        recomputeFixed()
    }

    onWidthChanged: { recomputeFixed(); reportGeometry() }
    onHeightChanged: reportGeometry()

    Connections {
        target: DockModel
        function onLayout_changed() { Qt.callLater(dock.reportGeometry) }
    }

    Row {
        id: columnRow
        anchors.fill: parent
        spacing: 0

        Repeater {
            id: columns
            model: DockModel.columns

            delegate: Item {
                id: column
                objectName: "dockColumn"
                required property var modelData
                required property int index

                readonly property bool isStretch: modelData.indexOf("center") >= 0

                // The width a drag has put this column at. The conversation
                // ignores it and absorbs the slack instead, so the row always
                // fills the dock exactly.
                property int userWidth: DockModel.column_width(modelData[0])

                // What the column is actually given: what it asked for, less
                // whatever share it has to hand back when the row is over
                // budget, and never below its own floor.
                readonly property int paintedWidth:
                    dock.fitted[index] !== undefined ? dock.fitted[index] : userWidth

                width: isStretch ? dock.stretchWidth : paintedWidth
                height: parent.height

                onUserWidthChanged: dock.recomputeFixed()
                Component.onCompleted: dock.recomputeFixed()
                onIsStretchChanged: dock.recomputeFixed()

                onWidthChanged: Qt.callLater(dock.reportGeometry)

                Column {
                    anchors.fill: parent
                    spacing: 0

                    Repeater {
                        model: column.modelData
                        delegate: DockPanel {
                            required property var modelData
                            key: modelData
                            title: dock.titles[modelData] || modelData
                            draggable: dock.anchored.indexOf(modelData) < 0
                            dragging: DockModel.dragging === modelData
                            width: column.width
                            // A stacked column splits its height evenly.
                            height: column.height / column.modelData.length

                            onDragStarted: function (pos) {
                                DockModel.begin_drag(key, pos.x, pos.y)
                            }
                            onDragMoved: function (pos) {
                                const local = dock.mapFromGlobal(pos.x, pos.y)
                                DockModel.update_drag(local.x, local.y)
                            }
                            onDragEnded: function (pos) {
                                const local = dock.mapFromGlobal(pos.x, pos.y)
                                DockModel.end_drag(local.x, local.y)
                            }

                            // Adopting a panel is a reparent, never a rebuild:
                            // a panel destroyed and recreated on every re-dock
                            // would lose its terminals, its browser and its
                            // scroll position.
                            Component.onCompleted: adopt()
                            onKeyChanged: adopt()

                            function adopt() {
                                const content = dock.panels[key]
                                if (!content)
                                    return
                                content.parent = contentArea
                                content.anchors.fill = contentArea
                                // A panel that carries tabs hands them up into
                                // the header, so it wears one strip rather than
                                // a title bar with a second bar under it.
                                headerTabs = content.tabStrip !== undefined
                                             ? content.tabStrip : null
                                headerTools = content.headerTools !== undefined
                                              ? content.headerTools : null
                            }
                        }
                    }
                }

                // The divider on this column's right edge, drawn by the column
                // rather than between them so the last one simply has none.
                //
                // Each half is painted from the panel it touches, not from one
                // colour for both: the only panel with a surface of its own is
                // History, and a divider beside it in the card colour would put
                // a stripe of the wrong shade against it. What is left is the
                // hairline down the middle — the border those two panels no
                // longer draw themselves.
                Item {
                    id: divider
                    visible: column.index < columns.count - 1
                    anchors { right: parent.right; top: parent.top; bottom: parent.bottom }
                    width: dock.dividerWidth

                    // A divider moves the boundary between the two columns it
                    // sits between, and only those two — width taken from one
                    // is given to the other.
                    //
                    // Anything else makes a divider between two side panels
                    // useless: it would be trying to take its width from the
                    // conversation, which is usually already at its floor and
                    // has none to give, while the neighbour it is actually
                    // touching sits there with room to spare.
                    //
                    // Between History and the conversation neither side may
                    // move, and the divider is inert — which is what keeps the
                    // sidebar at its fixed width.
                    readonly property var rightKeys: DockModel.columns[column.index + 1] || []
                    readonly property string leftKey: column.modelData[0]
                    readonly property string rightKey:
                        rightKeys.length > 0 ? rightKeys[0] : ""
                    readonly property bool canResize:
                        DockModel.resizable(leftKey)
                        || (rightKey !== "" && DockModel.resizable(rightKey))

                    Repeater {
                        model: column.modelData
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            x: 0
                            y: index * (parent.height / column.modelData.length)
                            width: dock.dividerWidth / 2
                            height: parent.height / column.modelData.length
                            color: dock.surfaceOf(modelData)
                        }
                    }

                    Repeater {
                        model: DockModel.columns[column.index + 1] || []
                        delegate: Rectangle {
                            required property var modelData
                            required property int index
                            readonly property int rows:
                                (DockModel.columns[column.index + 1] || []).length
                            x: dock.dividerWidth / 2
                            y: index * (parent.height / Math.max(1, rows))
                            width: dock.dividerWidth - x
                            height: parent.height / Math.max(1, rows)
                            color: dock.surfaceOf(modelData)
                        }
                    }

                    Rectangle {
                        anchors.centerIn: parent
                        width: 1
                        height: parent.height
                        color: Theme.colors.card_border
                    }

                    // The grab area is wider than the line it draws: a 5px
                    // target is a hairline to aim at, and the panels on either
                    // side have nothing at their edges to hit by mistake.
                    MouseArea {
                        anchors.fill: parent
                        anchors.leftMargin: -3
                        anchors.rightMargin: -3
                        enabled: divider.canResize
                        cursorShape: divider.canResize ? Qt.SplitHCursor
                                                       : Qt.ArrowCursor

                        property real pressX: 0
                        property int startLeft: 0
                        property int startRight: 0
                        property int startStretch: 0

                        onPressed: function (mouse) {
                            // Settle the automatic squeeze into real widths
                            // first. While it is in force a column is painted
                            // at a fraction of what it asked for, and a drag
                            // that moved the asked-for number would slide the
                            // divider at a fraction of the pointer's speed.
                            dock.bakeWidths()

                            const left = columns.itemAt(column.index)
                            const right = columns.itemAt(column.index + 1)
                            pressX = mapToItem(dock, mouse.x, 0).x
                            startLeft = left && !left.isStretch ? left.userWidth : 0
                            startRight = right && !right.isStretch ? right.userWidth : 0
                            startStretch = dock.stretchWidth
                        }

                        onPositionChanged: function (mouse) {
                            const left = columns.itemAt(column.index)
                            const right = columns.itemAt(column.index + 1)
                            if (!pressed || !left || !right)
                                return

                            const travel = mapToItem(dock, mouse.x, 0).x - pressX
                            const centreFloor = DockModel.min_width("center")

                            // Whichever side is the conversation answers with
                            // the stretch width, since it has no stored width
                            // of its own to give from.
                            const leftWidth = left.isStretch ? startStretch : startLeft
                            const leftFloor = left.isStretch
                                            ? centreFloor
                                            : DockModel.min_width(divider.leftKey)
                            const rightWidth = right.isStretch ? startStretch : startRight
                            const rightFloor = right.isStretch
                                             ? centreFloor
                                             : DockModel.min_width(divider.rightKey)

                            const step = DockModel.resize_step(
                                travel, leftWidth, leftFloor, rightWidth, rightFloor)

                            // Only the columns with a width of their own are
                            // written; the conversation follows from them.
                            if (!left.isStretch)
                                left.userWidth = startLeft + step
                            if (!right.isStretch)
                                right.userWidth = startRight - step
                        }

                        onReleased: {
                            const left = columns.itemAt(column.index)
                            const right = columns.itemAt(column.index + 1)
                            if (left && !left.isStretch)
                                DockModel.set_column_width(divider.leftKey,
                                                           left.userWidth)
                            if (right && !right.isStretch)
                                DockModel.set_column_width(divider.rightKey,
                                                           right.userWidth)
                        }
                    }
                }
            }
        }
    }

    // Drop indicator: a translucent fill with a solid accent border. One colour
    // for both themes — it is painted over whichever panels the drag is passing
    // across, so it answers to those rather than to the theme, and the accent is
    // legible on either.
    Rectangle {
        visible: DockModel.drop_visible
        x: DockModel.drop_x
        y: DockModel.drop_y
        width: DockModel.drop_width
        height: DockModel.drop_height
        color: Qt.rgba(0.29, 0.43, 0.81, 0.24)
        border.color: Qt.rgba(0.29, 0.43, 0.81, 0.86)
        border.width: 2
        radius: 4
    }
}
