import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import "../common"

// The Files pane: the workspace's tree, the way an editor draws one.
//
// One flat list rather than nested views. The bridge flattens the tree to rows
// carrying their own depth, so a branch opening is one list changing length
// instead of a view being built inside a view — which is what keeps a thousand
// rows scrolling at the same speed as ten, and what lets the whole tree be
// tested without a display.
//
// Files come in and out of the project by drag, by the chooser, or from the row
// menu, and all three end in the same two bridge calls. Nothing here decides
// whether a copy is allowed or what it lands called: those are the rules that
// write to someone's disk, and they live in the core where they are tested.
Item {
    id: panel

    // Mounted into the dock's panel header; see DockPanel.qml.
    property Item headerTools: tools

    // A copy already running, or one stopped on a question nobody has answered
    // yet, is a copy in flight as far as starting another one goes.
    readonly property bool canCopy: !Files.busy && !Files.collision_open

    readonly property int rowHeight: 22
    readonly property int indentStep: 14

    Item {
        id: toolsHolder
        visible: false

        Row {
            id: tools
            spacing: 2

            IconButton {
                glyph: "plus"
                glyphSize: 14
                tooltip: qsTr("Copy files into the workspace")
                enabled: panel.canCopy
                onClicked: {
                    importDialog.destination = panel.destinationForImport()
                    importDialog.open()
                }
            }

            IconButton {
                glyph: "ellipsis"
                glyphSize: 14
                tooltip: qsTr("Options")
                onClicked: optionsMenu.popup()

                Menu {
                    id: optionsMenu
                    MenuItem {
                        id: hiddenItem
                        text: qsTr("Show hidden files")
                        checkable: true
                        checked: Files.show_hidden
                        onTriggered: Files.show_hidden = checked

                        // Clicking the item writes `checked` itself, which
                        // destroys the binding above. The side strip's toggles
                        // have the same problem and the same answer.
                        Connections {
                            target: Files
                            function onRows_changed() {
                                hiddenItem.checked = Files.show_hidden
                            }
                        }
                    }
                    MenuItem {
                        text: qsTr("Collapse all")
                        onTriggered: Files.collapse_all()
                    }
                }
            }

            // See DiffPanel: a character, because the icon set has no reload
            // glyph.
            TextButton {
                text: "↻"
                fontSize: 14
                tooltip: qsTr("Refresh")
                onClicked: Files.refresh()
            }
        }
    }

    // Nothing to list: no workspace, a remote one, an empty folder, or a folder
    // that would not be read. Which of those it was is the bridge's sentence —
    // "empty" and "no permission" look identical in a list and mean opposite
    // things.
    Text {
        anchors { fill: parent; margins: 12 }
        visible: Files.message !== ""
        text: Files.message
        color: Files.dim_color
        font.family: Theme.sans_family
        font.pixelSize: 11
        wrapMode: Text.Wrap
    }

    ListView {
        id: list
        anchors {
            left: parent.left; right: parent.right; top: parent.top
            bottom: capNotice.top
            leftMargin: 6; rightMargin: 6; topMargin: 6
        }
        visible: Files.message === ""
        clip: true
        model: Files.rows
        boundsBehavior: Flickable.StopAtBounds
        // A tree is read by name, and the names are what runs off the right
        // edge. Horizontal travel would fight the drag-out gesture for the same
        // pointer, so deep rows elide instead.
        flickableDirection: Flickable.VerticalFlick

        ScrollBar.vertical: ThinScrollBar {}

        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index

            readonly property bool selected: Files.selected === modelData.path
            // The folder this drop would land in — which for a file row is the
            // folder holding it, so the row that lights up is the one the copy
            // actually goes into.
            readonly property bool dropTarget: dropZone.targetPath !== ""
                                               && dropZone.targetPath === modelData.path

            width: list.width
            height: panel.rowHeight
            radius: 4
            color: row.dropTarget ? Theme.colors.accent_soft
                 : row.selected ? Theme.colors.accent_soft
                 : rowMouse.containsMouse ? Theme.colors.hover
                 : "transparent"

            // The twisty. A vendored glyph rather than a triangle character:
            // the bundled mono face has no ▸ and drew it as a dot, which is
            // exactly the kind of thing that only shows up in a picture.
            Image {
                id: twisty
                anchors {
                    left: parent.left
                    leftMargin: 4 + row.modelData.depth * panel.indentStep
                    verticalCenter: parent.verticalCenter
                }
                width: 11
                height: 11
                sourceSize: Qt.size(22, 22)
                smooth: true
                visible: row.modelData.is_dir && !row.modelData.loading
                source: Theme.icon(
                    row.modelData.expanded ? "chevron-down" : "chevron-right",
                    Files.dim_color)
            }

            // An open folder whose listing has not arrived. Only ever visible
            // on a remote workspace, which is exactly where a branch that took
            // a moment would otherwise read as a click that did nothing.
            Text {
                anchors {
                    left: parent.left
                    leftMargin: 4 + row.modelData.depth * panel.indentStep
                    verticalCenter: parent.verticalCenter
                }
                width: 11
                horizontalAlignment: Text.AlignHCenter
                visible: row.modelData.loading
                text: "·"
                color: Files.dim_color
                font.family: Theme.mono_family
                font.pixelSize: 11
            }

            Image {
                id: glyph
                anchors {
                    // Off the row's own left edge, not off the twisty: an
                    // invisible item still has a position, and anchoring to it
                    // would work, but saying the indent once is what keeps a
                    // file and the folder above it in the same column.
                    left: parent.left
                    leftMargin: 4 + row.modelData.depth * panel.indentStep + 13
                    verticalCenter: parent.verticalCenter
                }
                width: 13
                height: 13
                sourceSize: Qt.size(26, 26)
                smooth: true
                source: Theme.icon(
                    row.modelData.is_dir
                        ? (row.modelData.expanded ? "folder-open" : "folder")
                        : "file",
                    row.selected ? Theme.colors.accent_text
                                 : Files.dim_color)
            }

            Text {
                anchors {
                    left: glyph.right; leftMargin: 6
                    right: size.left; rightMargin: 8
                    verticalCenter: parent.verticalCenter
                }
                text: row.modelData.name
                color: row.selected ? Theme.colors.accent_text : Theme.colors.text
                font.family: Theme.sans_family
                font.pixelSize: 11
                // A symlink is worth saying so in the one way that costs no
                // room: the name leans.
                font.italic: row.modelData.symlink
                elide: Text.ElideRight
            }

            Text {
                id: size
                anchors {
                    right: parent.right; rightMargin: 6
                    verticalCenter: parent.verticalCenter
                }
                text: row.modelData.size_label
                color: Files.dim_color
                font.family: Theme.mono_family
                font.pixelSize: 10
            }

            // What a drag out of the panel carries. `Drag.Automatic` is what
            // makes it a drag the rest of the desktop can receive rather than
            // one that only means something inside this window.
            //
            // Offered for a local workspace only. What the desktop's drag
            // protocol wants is a path another application can open, and a file
            // on the far side of an SSH connection has none until it has been
            // fetched — which cannot happen inside the gesture. "Copy out of
            // the workspace…" in the row menu does the same job with a
            // destination chosen first and the transfer on a worker.
            Item {
                id: payload
                Drag.active: rowMouse.drag.active
                Drag.dragType: Drag.Automatic
                Drag.supportedActions: Qt.CopyAction
                Drag.mimeData: ({ "text/uri-list": Files.url_for(row.modelData.path) })
            }

            MouseArea {
                id: rowMouse
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                drag.target: Files.remote ? null : payload
                // Past the platform's own threshold, so a click that wanders a
                // pixel is still a click.
                drag.threshold: 8

                onClicked: function (event) {
                    Files.selected = row.modelData.path
                    if (event.button === Qt.RightButton) {
                        rowMenu.popup()
                        return
                    }
                    // A folder opens in place; a file opens the sheet. One
                    // click for both, because "click a folder to open it" and
                    // "click a file to read it" are the same gesture to
                    // whoever is pointing at the row.
                    if (row.modelData.is_dir)
                        Files.toggle(row.modelData.path)
                    else
                        Files.open_preview(row.modelData.path)
                }
            }

            Menu {
                id: rowMenu
                MenuItem {
                    text: qsTr("Preview")
                    enabled: !row.modelData.is_dir
                    onTriggered: Files.open_preview(row.modelData.path)
                }
                MenuItem {
                    text: qsTr("Copy out of the workspace…")
                    enabled: panel.canCopy
                    onTriggered: {
                        exportDialog.source = row.modelData.path
                        exportDialog.open()
                    }
                }
                MenuItem {
                    text: qsTr("Copy files in here…")
                    enabled: panel.canCopy && row.modelData.is_dir
                    onTriggered: {
                        importDialog.destination = row.modelData.path
                        importDialog.open()
                    }
                }
                MenuSeparator {}
                MenuItem {
                    text: qsTr("Copy path")
                    onTriggered: clipboard.copy(row.modelData.path)
                }
            }
        }
    }

    // The whole panel takes drops, rather than each row taking its own: nested
    // drop areas hand the event around between themselves at the boundaries,
    // and the row under the pointer is a lookup either way.
    DropArea {
        id: dropZone
        anchors.fill: parent

        // The folder the drop would land in, as a path. Empty when there is
        // nothing under the pointer to answer for.
        property string targetPath: ""

        function targetAt(x, y) {
            if (Files.message !== "")
                return ""
            const local = mapToItem(list.contentItem, x, y)
            const index = list.indexAt(local.x, local.y)
            // Off the end of the list is the workspace itself, which is what an
            // empty path means to the bridge.
            const path = index < 0 ? "" : Files.rows[index].path
            return Files.drop_target(path)
        }

        onEntered: function (drag) {
            // A drag carrying no files — a link out of a browser, a selection
            // out of an editor — has nothing to copy, and saying so at the
            // border is better than refusing it after the drop.
            if (!drag.hasUrls || !panel.canCopy || Files.message !== "") {
                drag.accepted = false
                return
            }
            targetPath = targetAt(drag.x, drag.y)
        }
        onPositionChanged: function (drag) {
            targetPath = targetAt(drag.x, drag.y)
        }
        onExited: targetPath = ""
        onDropped: function (drop) {
            const destination = targetAt(drop.x, drop.y)
            targetPath = ""
            if (destination === "")
                return
            Files.copy_in(drop.urls.join("\n"), destination)
            drop.acceptProposedAction()
        }
    }

    // The panel's own edge while a drop is over it. Drawn over the list rather
    // than around it so it reads as the panel accepting, not as a row.
    Rectangle {
        anchors.fill: parent
        visible: dropZone.containsDrag
        color: "transparent"
        border.width: 2
        border.color: Theme.colors.accent
        radius: 4
    }

    // What the last copy did. It stays until the next one replaces it: a line
    // that cleared itself after a moment is one that is gone by the time
    // anybody looks away from the folder they were watching.
    Rectangle {
        id: footer
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: message.text === "" ? 0 : 24
        visible: height > 0
        color: Theme.colors.header

        Rectangle {
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 1
            color: Theme.colors.card_border
        }

        Text {
            id: message
            anchors {
                left: parent.left; right: parent.right
                leftMargin: 8; rightMargin: 8
                verticalCenter: parent.verticalCenter
            }
            text: Files.status
            color: Files.status_failed ? Files.alert_color : Files.dim_color
            font.family: Theme.mono_family
            font.pixelSize: 10
            elide: Text.ElideMiddle
        }
    }

    // A tree that stopped at the row cap. Said out loud, because a list that is
    // simply short looks like a folder that is simply small — and said on a
    // strip of its own, because a sentence floating over the rows it is about
    // reads as one of them.
    Rectangle {
        id: capNotice
        anchors { left: parent.left; right: parent.right; bottom: footer.top }
        height: Files.truncated ? 22 : 0
        visible: height > 0
        color: Theme.colors.header

        Rectangle {
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 1
            color: Theme.colors.card_border
        }

        Text {
            anchors {
                left: parent.left; right: parent.right
                leftMargin: 8; rightMargin: 8
                verticalCenter: parent.verticalCenter
            }
            text: qsTr("Too many files to show them all — collapse a folder.")
            color: Files.dim_color
            font.family: Theme.sans_family
            font.pixelSize: 10
            elide: Text.ElideRight
        }
    }

    Clipboard { id: clipboard }

    // ---- Choosers -------------------------------------------------------------

    // The folder a chooser-driven import lands in: the selected folder, the
    // folder holding the selected file, or the workspace.
    function destinationForImport() {
        return Files.drop_target(Files.selected)
    }

    FileDialog {
        id: importDialog
        property string destination
        title: qsTr("Copy files into the workspace")
        fileMode: FileDialog.OpenFiles
        onAccepted: Files.copy_in(selectedFiles.join("\n"), destination)
    }

    FolderDialog {
        id: exportDialog
        property string source
        title: qsTr("Copy out to…")
        onAccepted: Files.copy_out(source, selectedFolder)
    }

    // ---- Wiring ---------------------------------------------------------------

    onVisibleChanged: if (visible) Files.refresh()

    Binding { target: Files; property: "workspace"; value: App.current }
    Binding { target: Files; property: "theme"; value: Theme.name }

    // A file the agent wrote is a file that should be in the tree. The pane
    // re-reads on the turn ending rather than on every event: a running turn
    // touches the same files many times, and a tree that rebuilt under each
    // touch would jump while it was being read.
    Connections {
        target: Session
        function onStatus_changed() {
            if (panel.visible && !Session.busy)
                Files.refresh()
        }
    }
}
