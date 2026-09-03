import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "../common"

// The window's decoration, replacing the native one: the close/minimize/zoom
// lights at the far left, then the History toggle, then two stacked lines
// naming what is open — the focused conversation's title over the workspace
// folder and its git branch. Memory sits at the right. Dragging the bar moves
// the window; double-clicking zooms it.
//
// The bar sits on the base surface rather than on the shade the panel headers
// and the side strips wear: those run along one edge of the content and frame
// it, while this spans the whole window above all of it. The hairline along its
// bottom is what separates the two.
Rectangle {
    id: bar

    property int cornerRadius: 0
    property bool maximized: false
    property string branch: ""
    property string memoryLabel: ""

    signal minimizeRequested()
    signal maximizeRequested()
    signal closeRequested()
    signal moveRequested()
    signal memoryRequested()

    // Two lines of text, so taller than the one-line bar it replaces.
    height: 44
    color: Theme.colors.window

    // Only the top corners are this widget's; the bottom of the window belongs
    // to the two side strips.
    topLeftRadius: bar.cornerRadius
    topRightRadius: bar.cornerRadius

    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 1
        color: Theme.colors.card_border
    }

    // The actions History offers on a row, aimed at whatever conversation is
    // focused instead. Only a session pi has already written to disk has
    // anything to act on — one still in its first turn has no file and no id.
    Menu {
        id: sessionMenu

        readonly property string path: History.selected
        readonly property string id: path === "" ? "" : History.session_id_for(path)

        MenuItem {
            text: qsTr("No saved session")
            enabled: false
            visible: sessionMenu.id === ""
            height: visible ? implicitHeight : 0
        }
        MenuItem {
            text: qsTr("Archive")
            visible: sessionMenu.id !== ""
            height: visible ? implicitHeight : 0
            onTriggered: History.archive(sessionMenu.path)
        }
        MenuItem {
            text: qsTr("Delete")
            visible: sessionMenu.id !== ""
            height: visible ? implicitHeight : 0
            onTriggered: History.remove(sessionMenu.path)
        }
    }

    // Hand the folder to another application. Filled as it opens, from the one
    // list that also knows how to launch each of them — a menu that named an
    // application the launcher did not would be a menu entry that does nothing.
    Menu {
        id: externalMenu
        onAboutToShow: {
            while (count > 0)
                takeItem(0)
            for (const name of App.external_apps()) {
                addItem(externalItem.createObject(externalMenu, {
                    text: qsTr("Open in %1").arg(name),
                    application: name
                }))
            }
        }
    }

    Component {
        id: externalItem
        MenuItem {
            property string application
            onTriggered: App.open_externally(application)
        }
    }

    // The repo's local branches; picking one checks it out. Filled as it opens,
    // because a branch list read at startup is a branch list that is wrong by
    // the time anyone looks at it.
    Menu {
        id: branchMenu
        onAboutToShow: {
            while (count > 0)
                takeItem(0)
            const branches = App.branches()
            for (const name of branches) {
                const item = branchItem.createObject(branchMenu, {
                    text: name === bar.branch ? "● " + name : "   " + name,
                    branchName: name
                })
                addItem(item)
            }
        }
    }

    Component {
        id: branchItem
        MenuItem {
            property string branchName
            onTriggered: {
                const error = App.checkout(branchName)
                if (error !== "") {
                    checkoutError.text = error
                    checkoutError.open()
                }
            }
        }
    }

    Dialog {
        id: checkoutError
        property alias text: message.text
        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Could not switch branch")
        standardButtons: Dialog.Ok
        Text {
            id: message
            color: Theme.colors.text
            font.family: Theme.sans_family
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }
    }

    // Dragging the bar moves the window and double-clicking it zooms, and the
    // two must not both start from the press. Handing the press straight to the
    // window manager gives it the pointer grab, so the second click of a double
    // never arrives here — and the interactive move it begins un-fills a
    // maximized window on the way, leaving a size neither gesture asked for. So
    // the move waits until the pointer has actually travelled.
    MouseArea {
        anchors.fill: parent

        property point origin
        property bool moving: false

        onPressed: function (event) {
            origin = Qt.point(event.x, event.y)
            moving = false
        }
        onPositionChanged: function (event) {
            if (moving || !pressed)
                return
            const travelled = Math.hypot(event.x - origin.x, event.y - origin.y)
            if (travelled < Application.styleHints.startDragDistance)
                return
            moving = true
            bar.moveRequested()
        }
        onDoubleClicked: bar.maximizeRequested()
    }

    RowLayout {
        anchors.fill: parent
        // The lights sit further from the left edge than the rest of the bar's
        // contents: they are what the window's rounded corner curves around.
        anchors.leftMargin: 14
        anchors.rightMargin: 8
        spacing: 6

        // Close, minimize, zoom — the platform's order, at the platform's
        // spacing, which is wider than this bar's own.
        Row {
            id: lights
            property bool hovered: closeLight.hovered || minLight.hovered || maxLight.hovered
            spacing: 8
            Layout.rightMargin: 8

            TrafficLight {
                id: closeLight
                property bool hovered: hover.hovered
                kind: "close"
                tooltip: qsTr("Close")
                groupHovered: lightsHover.hovered
                onClicked: bar.closeRequested()
                HoverHandler { id: hover }
            }
            TrafficLight {
                id: minLight
                property bool hovered: minHover.hovered
                kind: "min"
                tooltip: qsTr("Minimize")
                groupHovered: lightsHover.hovered
                onClicked: bar.minimizeRequested()
                HoverHandler { id: minHover }
            }
            TrafficLight {
                id: maxLight
                property bool hovered: maxHover.hovered
                kind: bar.maximized ? "restore" : "max"
                tooltip: bar.maximized ? qsTr("Restore") : qsTr("Zoom")
                groupHovered: lightsHover.hovered
                onClicked: bar.maximizeRequested()
                HoverHandler { id: maxHover }
            }
            HoverHandler { id: lightsHover }
        }

        IconButton {
            id: historyToggle
            glyph: "panel-left"
            tooltip: qsTr("Toggle History Panel")
            visible: App.current !== ""
            // Implicit, not `width`: a layout sizes a row from what its
            // children *ask* for, and a plain width leaves the ask at the
            // default 28 — which is what pushed this bar's second line off the
            // bottom of it.
            implicitWidth: 24
            implicitHeight: 24
            glyphSize: 16
            checkable: true
            // The History panel's toggle answers the same question the side
            // strip's buttons do, so it is marked in the same blue — softened,
            // because it sits among text rather than in a strip of its own.
            checkedColor: Theme.colors.accent_soft
            checkedGlyphColor: Theme.colors.accent_text
            checked: App.is_panel_visible("left")
            onClicked: App.set_panel_visible("left", checked)

            Connections {
                target: App
                function onPanels_changed() {
                    historyToggle.checked = App.is_panel_visible("left")
                }
                function onCurrent_changed() {
                    historyToggle.checked = App.is_panel_visible("left")
                }
            }
        }

        // What is open, on two lines: its name, then where it lives. The rows
        // are a column of their own so both start at the same left edge.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 0

            RowLayout {
                spacing: 4
                Text {
                    text: App.conversation !== "" ? App.conversation
                                                  : qsTr("No session selected")
                    color: Theme.colors.text
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.chat_font_size
                    font.weight: Font.DemiBold
                    elide: Text.ElideRight
                    Layout.maximumWidth: 520
                }
                IconButton {
                    id: sessionButton
                    glyph: "ellipsis"
                    tooltip: qsTr("Session actions")
                    visible: App.current !== ""
                    implicitWidth: 20
                    implicitHeight: 20
                    glyphSize: 14
                    onClicked: sessionMenu.popup()
                }
                Item { Layout.fillWidth: true }
            }

            RowLayout {
                spacing: 4
                Text {
                    text: App.workspace_label
                    color: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
                    font.family: Theme.sans_family
                    font.pixelSize: 11
                    elide: Text.ElideLeft
                    Layout.maximumWidth: 460
                }
                BranchChip {
                    id: branchChip
                    branch: bar.branch
                    visible: bar.branch !== ""
                    onClicked: branchMenu.popup()
                }
                Item { Layout.fillWidth: true }
            }
        }

        // Right-hand chrome: what the workspace can be opened in, then what the
        // app is costing. Both belong to the window rather than to the session
        // named on the left, which is why they sit at the far end together.
        ExternalOpenButton {
            id: externalButton
            visible: App.current !== ""
            enabled: App.can_open_externally()
            onClicked: externalMenu.popup()
        }

        Text {
            text: bar.memoryLabel
            visible: bar.memoryLabel !== ""
            color: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
            font.family: Theme.sans_family
            font.pixelSize: 11
            padding: 3
            MouseArea {
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: bar.memoryRequested()
                ToolTipLabel {
                    text: qsTr("Show Kraken processes and memory")
                    visible: parent.containsMouse
                }
            }
        }
    }
}
