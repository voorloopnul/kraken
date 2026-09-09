import QtQuick
import QtQuick.Controls
import "../common"

// The Changes tab of the Git pane: what has changed in this workspace's repo
// since the last commit, file by file — and the commit and push that publish it.
// GitPanel is what puts it on screen.
//
// One row per file with its own added and removed counts, not a split between
// staged and unstaged — the question the tab answers is "what has the agent
// been doing", and a change that is half staged is still one change. Every row
// is a click away from the full diff (see DiffViewer.qml), and a tick away from
// the next commit.
//
// Checking a file is the only thing that puts it in a commit: the index is not
// the selection, and a file staged from the terminal is committed here only if
// it is ticked here too. The checked set is the Diff bridge's, the write is the
// Git bridge's, and the paths travel from one to the other through the Commit
// button — which is also what keeps a click from committing a selection the
// reader can no longer see.
//
// The colours are the bridge's rather than this file's: a status letter, a
// deleted path and a zero count each have a colour that means something, and
// working that rule out twice is how the two copies of it drift.
Item {
    id: panel

    // The checked paths as a set: a delegate asks about one path, and asking a
    // list would be a scan per row per change.
    readonly property var checked: new Set(Diff.selected_paths)
    readonly property int checkedCount: Diff.selected_paths.length
    // Both halves of what a commit needs, and nothing running that it would
    // race. The Git bridge checks all of this again; this is what greys the
    // button rather than what enforces the rule.
    readonly property bool committable: Git.workspace !== ""
                                        && Git.workspace === Diff.workspace
                                        && !Git.action_busy && !Diff.loading
                                        && checkedCount > 0
                                        && Git.commit_message.trim() !== ""

    // Totals, and the only place the empty and failed states are said. No
    // placeholder row is faked into the list: a pane with one row in it that
    // is not a file reads as a file.
    Text {
        id: summary
        anchors { top: parent.top; left: parent.left; right: parent.right; margins: 10 }
        textFormat: Text.RichText
        text: Diff.summary
        font.family: Theme.mono_family
        font.pixelSize: 13
        wrapMode: Text.Wrap
    }

    // The whole selection in one line: the box that takes all of it or none of
    // it, and how much of it is taken. The count is on the right, where the
    // per-row counts are, rather than trailing the label it is not part of.
    Item {
        id: picker
        anchors {
            top: summary.bottom; topMargin: 8
            left: parent.left; right: parent.right
            leftMargin: 10; rightMargin: 12
        }
        height: 16
        visible: list.count > 0

        TickBox {
            id: allBox
            anchors { left: parent.left; leftMargin: 4; verticalCenter: parent.verticalCenter }
            checked: Diff.all_selected
            // Some but not all: the box says so rather than claiming either.
            partial: !Diff.all_selected && panel.checkedCount > 0
            enabled: !Git.action_busy && !Diff.loading
            tooltip: qsTr("Check every listed file")
            Accessible.name: qsTr("Select all changed files")
            onToggled: function (on) { Diff.select_all(on) }
        }

        Text {
            anchors { left: allBox.right; leftMargin: 8; verticalCenter: parent.verticalCenter }
            text: qsTr("Select all")
            color: Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: 12
        }

        Text {
            anchors { right: parent.right; verticalCenter: parent.verticalCenter }
            text: qsTr("%1 of %2 selected").arg(panel.checkedCount).arg(list.count)
            color: Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: 12
        }
    }

    ListView {
        id: list
        anchors {
            top: picker.visible ? picker.bottom : summary.bottom
            bottom: composer.top
            left: parent.left; right: parent.right
            topMargin: 6; bottomMargin: 8
            leftMargin: 10; rightMargin: 10
        }
        clip: true
        model: Diff.files
        boundsBehavior: Flickable.StopAtBounds

        ScrollBar.vertical: ThinScrollBar {}

        delegate: Rectangle {
            id: row
            required property var modelData
            required property int index

            width: list.width
            height: 22
            radius: 4
            color: rowMouse.containsMouse ? Theme.colors.hover : "transparent"

            TickBox {
                id: tick
                anchors { left: parent.left; leftMargin: 4; verticalCenter: parent.verticalCenter }
                // A binding, not a state of its own: the bridge is what says
                // whether this file is in the next commit, and a box that
                // answered for itself would drift from it.
                checked: panel.checked.has(row.modelData.path)
                enabled: !Git.action_busy && !Diff.loading
                tooltip: qsTr("Include this file in the commit")
                Accessible.name: qsTr("Include %1 in commit").arg(row.modelData.path)
                onToggled: function (on) { Diff.set_selected(row.modelData.path, on) }
            }

            Text {
                id: letter
                anchors { left: tick.right; leftMargin: 8; verticalCenter: parent.verticalCenter }
                width: 12
                text: row.modelData.letter
                color: row.modelData.letter_color
                font.family: Theme.mono_family
                font.pixelSize: 13
                font.weight: Font.DemiBold
            }

            Text {
                anchors {
                    left: letter.right; leftMargin: 4
                    right: counts.left; rightMargin: 8
                    verticalCenter: parent.verticalCenter
                }
                text: row.modelData.path
                color: row.modelData.path_color
                font.family: Theme.mono_family
                font.pixelSize: 13
                // Eat into the leading directories: a path's tail is the
                // informative end, and hiding the filename hides the row.
                elide: Text.ElideLeft
            }

            Row {
                id: counts
                // Clear of the scrollbar, which overlays the list's right
                // edge rather than taking a column out of it.
                anchors { right: parent.right; rightMargin: 12; verticalCenter: parent.verticalCenter }
                spacing: 6

                Text {
                    text: row.modelData.adds
                    color: row.modelData.adds_color
                    font.family: Theme.mono_family
                    font.pixelSize: 13
                }
                Text {
                    text: row.modelData.dels
                    color: row.modelData.dels_color
                    font.family: Theme.mono_family
                    font.pixelSize: 13
                }
            }

            // Everything right of the tick opens the diff. The box keeps its
            // own clicks: choosing a file to commit and reading it are two
            // different intentions, and one row cannot guess between them.
            MouseArea {
                id: rowMouse
                anchors { fill: parent; leftMargin: tick.width + 8 }
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                acceptedButtons: Qt.LeftButton | Qt.RightButton
                onClicked: function (event) {
                    if (event.button === Qt.RightButton) {
                        rowMenu.popup()
                        return
                    }
                    Diff.open_file(row.index)
                }

                ToolTipLabel {
                    text: row.modelData.tooltip
                    visible: rowMouse.containsMouse && row.modelData.tooltip !== ""
                }
            }

            Menu {
                id: rowMenu
                MenuItem {
                    text: qsTr("Copy path")
                    onTriggered: clipboard.copy(Diff.path_at(row.index))
                }
                MenuItem {
                    text: qsTr("Open diff")
                    onTriggered: Diff.open_file(row.index)
                }
            }
        }
    }

    // ---- The commit ------------------------------------------------------------

    // The message, the two buttons, and whatever git last said. It sits at the
    // bottom whatever the list is doing, so the reader can type while the list
    // is still being read: the file list is what gives up height, never this.
    Column {
        id: composer
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: 10 }
        spacing: 6

        Rectangle {
            width: parent.width
            height: 1
            color: Theme.colors.card_border
        }

        Rectangle {
            width: parent.width
            // Two lines and a bit: enough for a subject and the start of a
            // body, and it scrolls past that rather than eating the file list.
            height: 62
            radius: 6
            color: Theme.colors.card
            border.width: 1
            border.color: message.activeFocus ? Theme.colors.accent : Theme.colors.card_border

            Flickable {
                anchors { fill: parent; margins: 4 }
                contentWidth: width
                contentHeight: message.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                clip: true

                ScrollBar.vertical: ThinScrollBar {}

                TextArea.flickable: TextArea {
                    id: message
                    // Two-way with the Git bridge, which keeps one draft per
                    // workspace: switching projects and coming back finds the
                    // half-written message where it was left.
                    text: Git.commit_message
                    onTextChanged: if (text !== Git.commit_message) Git.commit_message = text
                    placeholderText: qsTr("Commit message")
                    Accessible.name: qsTr("Commit message")
                    color: Theme.chat_colors.text
                    placeholderTextColor: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: 12
                    wrapMode: TextArea.Wrap
                    selectByMouse: true
                    enabled: !Git.action_busy
                    background: null
                    padding: 2

                    // Enter breaks the line — a commit message has a body — so
                    // committing from the keyboard takes the modifier.
                    Keys.onReturnPressed: function (event) {
                        if (event.modifiers & Qt.ControlModifier) {
                            if (panel.committable)
                                Git.commit(Diff.workspace, Diff.selected_paths)
                            return
                        }
                        event.accepted = false
                    }
                }
            }
        }

        Text {
            width: parent.width
            text: qsTr("Commit includes only checked files, with all their changes.")
            color: Theme.chat_colors.dim
            font.family: Theme.sans_family
            font.pixelSize: 11
            wrapMode: Text.Wrap
        }

        Item {
            width: parent.width
            height: 22

            Row {
                anchors { right: parent.right; verticalCenter: parent.verticalCenter }
                spacing: 4

                TextButton {
                    text: qsTr("Commit")
                    fontSize: 12
                    enabled: panel.committable
                    tooltip: panel.checkedCount === 0
                             ? qsTr("Check the files to commit")
                             : qsTr("Commit the checked files only  ·  Ctrl+Enter")
                    onClicked: Git.commit(Diff.workspace, Diff.selected_paths)
                }

                TextButton {
                    text: qsTr("Push")
                    fontSize: 12
                    enabled: Git.workspace !== "" && !Git.action_busy
                    tooltip: qsTr("Push to where this repository already points")
                    onClicked: Git.push()
                }
            }
        }

        // Git's errors can be long — a hook's output, a rejected push — so they
        // are readable and selectable here rather than clipped to a line, and
        // they still cannot grow into the file list.
        Rectangle {
            width: parent.width
            height: visible ? Math.min(80, statusText.implicitHeight + 4) : 0
            visible: Git.action_status !== ""
            color: "transparent"

            Flickable {
                anchors.fill: parent
                contentWidth: width
                contentHeight: statusText.implicitHeight
                boundsBehavior: Flickable.StopAtBounds
                clip: true

                ScrollBar.vertical: ThinScrollBar {}

                TextArea.flickable: TextArea {
                    id: statusText
                    text: Git.action_status
                    textFormat: TextEdit.PlainText
                    Accessible.name: qsTr("Git operation status")
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextArea.Wrap
                    color: Git.action_error ? Theme.chat_colors.error : Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: 11
                    background: null
                    padding: 0
                }
            }
        }
    }

    Clipboard { id: clipboard }

    // ---- Wiring ---------------------------------------------------------------

    // Refreshed on first sight rather than on a timer: git is a subprocess (an
    // SSH round trip on a remote workspace), and a tab nobody is looking at is
    // not worth one. Behind the Commits tab this is invisible and so silent,
    // and coming back to it is what re-reads the repository.
    onVisibleChanged: if (visible) Diff.refresh()

    Binding { target: Diff; property: "theme"; value: Theme.name }

    // A commit or a checkout changes what "since the last commit" means, so the
    // tab's whole answer changes with HEAD. A write that failed can have
    // changed the index on its way to failing, which is why the result of one
    // is worth a refresh whichever way it went.
    Connections {
        target: Git
        function onBranch_changed() { if (panel.visible) Diff.refresh() }
        function onRepository_changed() { if (panel.visible) Diff.refresh() }
    }

    Connections {
        target: Diff
        function onWorkspace_changed() {
            // Defer until the workspace setter has released the bridge.
            if (panel.visible) Qt.callLater(function () { Diff.refresh() })
        }
    }
}
