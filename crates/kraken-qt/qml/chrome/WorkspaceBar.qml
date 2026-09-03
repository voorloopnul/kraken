import QtQuick
import QtQuick.Controls
import "../common"

// The strip down the window's left edge: a "+" to add a workspace, one coloured
// tile per open workspace, and the app's own actions pinned to the bottom.
//
// The same strip the panel toggles live in on the other side of the window, so
// it wears the same surface and the same quiet grey. The workspace tiles in
// between are the exception — each carries a colour of its own.
Rectangle {
    id: bar

    property int cornerRadius: 0

    signal addLocalRequested()
    signal addRemoteRequested()
    signal settingsRequested()
    signal quitRequested()
    signal editRemoteRequested(string anchor)

    width: 40
    color: Theme.colors.sidebar
    // The strip's own corner is the window's bottom-left.
    bottomLeftRadius: bar.cornerRadius

    Rectangle {
        anchors { top: parent.top; bottom: parent.bottom; right: parent.right }
        width: 1
        color: Theme.colors.card_border
    }

    // The bar down the strip's edge beside the current workspace. The tile's
    // own weight already says which workspace is current, but only against the
    // other tiles — with one workspace open there is nothing to compare it to,
    // and a lone saturated tile says nothing at all.
    Rectangle {
        id: indicator
        width: 3
        radius: 1.5
        color: App.indicator_color(Theme.name)
        visible: currentTile !== null
        property Item currentTile: null
        height: currentTile ? currentTile.height * 0.6 : 0
        x: 0
        y: currentTile ? currentTile.mapToItem(bar, 0, 0).y
                         + (currentTile.height - height) / 2
                       : 0
    }

    Column {
        anchors { top: parent.top; left: parent.left; right: parent.right }
        anchors.margins: 4
        anchors.topMargin: 8
        spacing: 8

        IconButton {
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "plus"
            tooltip: qsTr("Add Workspace")
            onClicked: addMenu.popup()
        }

        Repeater {
            model: App.workspaces
            delegate: WorkspaceTile {
                id: tile
                required property var modelData
                anchors.horizontalCenter: parent.horizontalCenter
                entry: modelData
                onClicked: App.select_workspace(modelData.key)
                onMenuRequested: tileMenu.popup()
                onCurrentChanged: if (current) indicator.currentTile = this
                Component.onCompleted: if (current) indicator.currentTile = this

                Menu {
                    id: tileMenu
                    MenuItem {
                        text: qsTr("Edit Remote Host…")
                        // Only a remote workspace has connection details to
                        // edit; a local folder is only ever its own path.
                        enabled: tile.modelData.remote
                        height: enabled ? implicitHeight : 0
                        visible: enabled
                        onTriggered: bar.editRemoteRequested(tile.modelData.key)
                    }
                    MenuItem {
                        text: qsTr("Remove from Workspaces")
                        onTriggered: App.remove_workspace(tile.modelData.key)
                    }
                }
            }
        }
    }

    // Pinned to the bottom edge, below whatever the workspace list reaches.
    Column {
        anchors { bottom: parent.bottom; left: parent.left; right: parent.right }
        anchors.margins: 4
        anchors.bottomMargin: 8
        spacing: 8

        IconButton {
            anchors.horizontalCenter: parent.horizontalCenter
            // Show the theme you would switch to: sun in dark mode, moon in light.
            glyph: Theme.name === "dark" ? "sun" : "moon"
            tooltip: qsTr("Toggle Theme")
            onClicked: Theme.toggle()
        }
        IconButton {
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "settings"
            tooltip: qsTr("Settings")
            onClicked: bar.settingsRequested()
        }
        IconButton {
            anchors.horizontalCenter: parent.horizontalCenter
            glyph: "power"
            tooltip: qsTr("Quit")
            onClicked: bar.quitRequested()
        }
    }

    // The "+" opens a menu: a local folder, or a remote SSH host.
    Menu {
        id: addMenu
        MenuItem {
            text: qsTr("Add Local Folder…")
            onTriggered: bar.addLocalRequested()
        }
        MenuItem {
            text: qsTr("Add Remote Host…")
            onTriggered: bar.addRemoteRequested()
        }
    }
}
