import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import "../common"

// The Changes tab of the Git pane: what has changed in this workspace's repo
// since the last commit, file by file. GitPanel is what puts it on screen.
//
// One row per file with its own added and removed counts, not a split between
// staged and unstaged — the question the tab answers is "what has the agent
// been doing", and a change that is half staged is still one change. Every row
// is a click away from the full diff (see DiffViewer.qml).
//
// The colours are the bridge's rather than this file's: a status letter, a
// deleted path and a zero count each have a colour that means something, and
// working that rule out twice is how the two copies of it drift.
Flickable {
    id: panel
    contentWidth: width
    contentHeight: layout.height + 20
    clip: true
    boundsBehavior: Flickable.StopAtBounds
    // Read once per selection change, not once per visible checkbox.
    readonly property var selectedPaths: Diff.selected_paths

    // Normally only the file list scrolls. A very short dock stack can be
    // smaller than the form itself; let the whole pane scroll in that case
    // rather than clipping the Commit and Push buttons out of reach.
    ScrollBar.vertical: ThinScrollBar {}

    component ActionButton: Button {
        id: control
        implicitHeight: 30
        font.family: Theme.mono_family
        font.pixelSize: 12
        opacity: enabled ? 1 : 0.45
        contentItem: Text {
            text: control.text
            font: control.font
            color: Theme.colors.text
            horizontalAlignment: Text.AlignHCenter
            verticalAlignment: Text.AlignVCenter
        }
        background: Rectangle {
            radius: 5
            color: control.down || control.hovered ? Theme.colors.hover : Theme.colors.header
            border.width: 1
            border.color: control.activeFocus ? Theme.colors.accent : Theme.colors.card_border
        }
    }

    component FileCheckBox: CheckBox {
        id: control
        implicitHeight: 24
        padding: 0
        spacing: 6
        enabled: !Git.action_busy && !Diff.loading
        opacity: enabled ? 1 : 0.45
        indicator: Rectangle {
            x: 4
            y: (control.height - height) / 2
            width: 14
            height: 14
            radius: 3
            color: control.checkState !== Qt.Unchecked ? Theme.colors.accent : Theme.colors.card
            border.width: 1
            border.color: control.activeFocus ? Theme.colors.accent : Theme.colors.card_border
            Text {
                anchors.centerIn: parent
                text: control.checkState === Qt.PartiallyChecked ? "−"
                      : control.checked ? "✓" : ""
                color: Theme.colors.accent_on
                font.pixelSize: 12
            }
        }
        contentItem: Text {
            text: control.text
            leftPadding: 24
            verticalAlignment: Text.AlignVCenter
            color: Theme.colors.text
            font.family: Theme.mono_family
            font.pixelSize: 11
            elide: Text.ElideRight
        }
    }

    ColumnLayout {
        id: layout
        x: 10
        y: 10
        width: panel.width - 20
        height: Math.max(panel.height - 20, implicitHeight)
        spacing: 6

        // Totals, and the only place the empty and failed states are said. No
        // placeholder row is faked into the list: a pane with one row in it that
        // is not a file reads as a file.
        Text {
            id: summary
            Layout.fillWidth: true
            textFormat: Text.RichText
            text: Diff.summary
            font.family: Theme.mono_family
            font.pixelSize: 13
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.fillWidth: true
            FileCheckBox {
                Layout.fillWidth: true
                text: qsTr("Select all")
                enabled: !Git.action_busy && !Diff.loading && list.count > 0
                tristate: true
                checkState: panel.selectedPaths.length === 0 ? Qt.Unchecked
                            : Diff.all_selected ? Qt.Checked : Qt.PartiallyChecked
                nextCheckState: function () {
                    return checkState === Qt.Checked ? Qt.Unchecked : Qt.Checked
                }
                onClicked: Diff.select_all(checkState === Qt.Checked)
            }
            Text {
                text: qsTr("%1 selected").arg(panel.selectedPaths.length)
                color: Theme.chat_colors.dim
                font.family: Theme.mono_family
                font.pixelSize: 11
            }
        }

        ListView {
            id: list
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 0
            Layout.preferredHeight: 0
            clip: true
            model: Diff.files
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}

            delegate: Rectangle {
                id: row
                required property var modelData
                required property int index

                width: list.width
                height: 24
                radius: 4
                color: rowMouse.containsMouse ? Theme.colors.hover : "transparent"

                FileCheckBox {
                    id: includeFile
                    anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                    width: 24
                    checked: panel.selectedPaths.indexOf(row.modelData.path) >= 0
                    Accessible.name: qsTr("Include %1 in commit").arg(row.modelData.path)
                    onClicked: Diff.set_selected(row.modelData.path, checked)
                }

                Text {
                    id: letter
                    anchors { left: includeFile.right; leftMargin: 4; verticalCenter: parent.verticalCenter }
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

                MouseArea {
                    id: rowMouse
                    // Leave the checkbox its own hit target. Clicking a name
                    // still opens the diff; checking it never opens a sheet.
                    anchors { left: includeFile.right; right: parent.right; top: parent.top; bottom: parent.bottom }
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

        // A fixed footer: the file list gives up height, not the message box,
        // so committing never requires scrolling past the changed files.
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 6

            Rectangle {
                Layout.fillWidth: true
                implicitHeight: 1
                color: Theme.colors.card_border
            }

            ScrollView {
                Layout.fillWidth: true
                Layout.preferredHeight: 68
                clip: true

                TextArea {
                    id: commitMessage
                    text: Git.commit_message
                    onTextChanged: if (text !== Git.commit_message) Git.commit_message = text
                    placeholderText: qsTr("Commit message")
                    Accessible.name: qsTr("Commit message")
                    enabled: !Git.action_busy
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    color: Theme.colors.text
                    placeholderTextColor: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: 12
                    padding: 8
                    background: Rectangle {
                        radius: 5
                        color: Theme.colors.card
                        border.width: 1
                        border.color: commitMessage.activeFocus ? Theme.colors.accent
                                                                : Theme.colors.card_border
                    }
                }
            }

            Text {
                Layout.fillWidth: true
                text: qsTr("Commit includes only checked files, with all their changes.")
                color: Theme.chat_colors.dim
                font.family: Theme.sans_family
                font.pixelSize: 11
                wrapMode: Text.Wrap
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 6

                ActionButton {
                    Layout.fillWidth: true
                    text: qsTr("Commit")
                    enabled: Git.workspace !== "" && Git.workspace === Diff.workspace
                             && !Git.action_busy && !Diff.loading && list.count > 0
                             && panel.selectedPaths.length > 0 && Git.commit_message.trim() !== ""
                    onClicked: Git.commit(Diff.workspace, panel.selectedPaths)
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Commit checked files only; unchecked staged files stay out")
                }
                ActionButton {
                    Layout.fillWidth: true
                    text: qsTr("Push")
                    enabled: Git.workspace !== "" && !Git.action_busy
                    onClicked: Git.push()
                    ToolTip.visible: hovered
                    ToolTip.text: qsTr("Push using Git's configured destination (no force)")
                }
            }

            // Git errors can be long (hooks, credentials, rejected pushes).
            // Keep them selectable and scrollable without swallowing the list.
            ScrollView {
                Layout.fillWidth: true
                Layout.preferredHeight: Math.min(80, statusText.implicitHeight)
                visible: Git.action_status !== ""
                clip: true

                TextArea {
                    id: statusText
                    text: Git.action_status
                    textFormat: TextEdit.PlainText
                    readOnly: true
                    selectByMouse: true
                    wrapMode: TextEdit.Wrap
                    color: Git.action_error ? Theme.chat_colors.error : Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: 11
                    padding: 0
                    background: null
                    Accessible.name: qsTr("Git operation status")
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
    // tab's whole answer changes with HEAD.
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
