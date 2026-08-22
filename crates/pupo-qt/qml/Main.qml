import QtQuick
import QtQuick.Window
import QtQuick.Dialogs
import "chrome"
import "common"
import "panels"
import "settings"

// The window and everything in it.
//
// The frame is ours: there is no native decoration, so the corner pixels belong
// to whichever child sits in them — the title bar along the top, the two side
// strips at the bottom — and each rounds its own outer corners to the same
// radius. A window filling the display is square instead: there is nothing
// beside it to round against, and a gap at the screen's own corner reads as a
// glitch.
Window {
    id: root

    title: "Pupo"
    width: Math.min(1440, Screen.desktopAvailableWidth)
    height: Math.min(900, Screen.desktopAvailableHeight)

    // The floor a drag on the edge grips stops at.
    //
    // Resizing is the window manager's, handed to it by `startSystemResize`, so
    // this is enforced by telling it the hint rather than by clamping anything
    // ourselves: Qt publishes these as the window's minimum size and the
    // compositor refuses to go under them.
    //
    // Below roughly this the dock has nothing left to give — History is fixed,
    // the conversation has a floor of its own, and a side panel or two are
    // already at theirs (see `pupo_core::dock`).
    minimumWidth: 900
    minimumHeight: 600
    visible: true
    flags: Qt.Window | Qt.FramelessWindowHint
    color: "transparent"

    // Every agent and every shell is a child process of ours; a window that
    // closed without reaping them would leave them running with nothing left to
    // read them. Done on `closing` rather than on destruction: by the time the
    // tree is being torn down the bridges may already be gone.
    onClosing: root.reapChildren()

    // Every agent and every shell is a child process of ours; a window that
    // closed without reaping them would leave them running with nothing left to
    // read them. Called from `closing` rather than from destruction: by the time
    // the tree is being torn down the bridges may already be gone — and from
    // the headless capture's own exit, which never raises `closing`.
    function reapChildren() {
        Session.shutdown()
        TerminalTabs.shutdown_all()
        Browser.discard_all()
    }

    readonly property bool fillsScreen: visibility === Window.Maximized
                                        || visibility === Window.FullScreen
    readonly property int cornerRadius: fillsScreen ? 0 : 10

    // JetBrains Mono is the face for the whole interface, so it looks the same
    // on every machine; Roboto covers the places a mono grid reads wrong.
    FontLoader { id: monoFont; source: "qrc:/assets/fonts/JetBrainsMono-Regular.ttf" }
    FontLoader { source: "qrc:/assets/fonts/JetBrainsMono-Bold.ttf" }
    FontLoader { source: "qrc:/assets/fonts/JetBrainsMono-Italic.ttf" }
    FontLoader { source: "qrc:/assets/fonts/JetBrainsMono-BoldItalic.ttf" }
    FontLoader { id: sansFont; source: "qrc:/assets/fonts/Roboto-Regular.ttf" }
    FontLoader { source: "qrc:/assets/fonts/Roboto-Bold.ttf" }
    FontLoader { source: "qrc:/assets/fonts/Roboto-Italic.ttf" }
    FontLoader { source: "qrc:/assets/fonts/Roboto-BoldItalic.ttf" }

    Binding {
        target: Theme; property: "mono_family"
        value: monoFont.status === FontLoader.Ready ? monoFont.name : "monospace"
    }
    Binding {
        target: Theme; property: "sans_family"
        value: sansFont.status === FontLoader.Ready ? sansFont.name : "sans-serif"
    }

    // The frame behind every child. With a transparent window, anything no
    // child paints comes out transparent — including the corner a rounded child
    // leaves open — so this carries the window colour under them all, rounded to
    // the same radius.
    Rectangle {
        id: frame
        anchors.fill: parent
        color: Theme.colors.window
        radius: root.cornerRadius

        Column {
            anchors.fill: parent
            spacing: 0

            TitleBar {
                id: titleBar
                width: parent.width
                cornerRadius: root.cornerRadius
                maximized: root.fillsScreen
                branch: App.branch
                memoryLabel: App.memory_label

                onMinimizeRequested: root.showMinimized()
                // Zoom, as the platform's own green button is — not maximize,
                // which is the *other* thing that button can do.
                onMaximizeRequested: root.fillsScreen ? root.showNormal()
                                                      : root.showFullScreen()
                onCloseRequested: root.close()
                onMoveRequested: root.startSystemMove()
                onMemoryRequested: processDialog.open()
            }

            Row {
                width: parent.width
                height: parent.height - titleBar.height
                spacing: 0

                WorkspaceBar {
                    id: workspaceBar
                    height: parent.height
                    cornerRadius: root.cornerRadius
                    onAddLocalRequested: folderDialog.open()
                    onAddRemoteRequested: Remotes.show("")
                    onEditRemoteRequested: (key) => Remotes.show(key)
                    onSettingsRequested: Settings.show()
                    onQuitRequested: Qt.quit()
                }

                Item {
                    id: content
                    width: parent.width - workspaceBar.width
                           - (sideBar.visible ? sideBar.width : 0)
                    height: parent.height

                    HomeScreen {
                        anchors.fill: parent
                        visible: App.current === ""
                        cornerRadius: root.cornerRadius
                    }

                    WorkspaceView {
                        id: workspace
                        anchors.fill: parent
                        visible: App.current !== ""
                    }
                }

                // The side strip acts on a workspace's panels, so it is hidden
                // on the home screen where there is nothing to toggle.
                SideBar {
                    id: sideBar
                    height: parent.height
                    visible: App.current !== ""
                    cornerRadius: root.cornerRadius
                    // The camera captures the browser's page, so it shows the
                    // panel first — a capture of a pane nobody has opened is a
                    // capture of nothing.
                    onScreenshotRequested: {
                        App.set_panel_visible("browser", true)
                        workspace.capturePage()
                    }
                }
            }
        }

        // A file's diff, over the whole window rather than inside the pane that
        // raised it: the sheet dims the app behind it, and a dimming that
        // stopped at one panel's edge would read as that panel having gone
        // dark. Inside the frame, so it is part of what a capture grabs and is
        // clipped to the window's own corners.
        DiffViewer {
            anchors.fill: parent
        }

        // Settings, over the app for the same reason and in the same way.
        SettingsWindow {
            anchors.fill: parent
        }

        ProcessDialog {
            id: processDialog
            anchors.fill: parent
        }

        RemoteDialog {
            anchors.fill: parent
        }
    }

    // Overlaid rather than laid out, so nothing else moves them. Dropped while
    // the window fills the display, where there is nothing to resize against.
    EdgeGrips {
        target: root
        visible: !root.fillsScreen
    }

    // One slow tick keeps the live readouts fresh: memory always drifts, and
    // the branch can change under us — a checkout in the terminal is the usual
    // way. Fast enough to notice, slow enough to cost nothing.
    Timer {
        interval: 3000
        repeat: true
        running: true
        onTriggered: App.refresh_chrome()
    }

    // The History pane follows whatever workspace is current.
    Connections {
        target: App
        function onCurrent_changed() { History.set_workspace(App.current) }
    }

    // A remote workspace that has just been added or edited: adding puts it in
    // the bar, and editing leaves it where it was but with a connection that
    // may now reach somewhere else.
    Connections {
        target: Remotes
        function onSaved(anchor) {
            App.add_workspace(anchor)
            App.select_workspace(anchor)
        }
    }


    FolderDialog {
        id: folderDialog
        title: qsTr("Add Workspace")
        onAccepted: App.add_workspace(selectedFolder.toString())
    }
}
