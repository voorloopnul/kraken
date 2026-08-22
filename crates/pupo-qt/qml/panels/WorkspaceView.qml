import QtQuick
import "../common"

// One workspace's panes.
//
// Every panel is built here, once, and handed to the dock to place. The dock
// only reparents them, so a panel's own state — terminals, a browser, a
// transcript's scroll position — survives a re-dock untouched.
Rectangle {
    id: view
    color: Theme.colors.card

    // The panels are built here and parked out of sight; the dock reparents
    // the ones it is showing into its own slots. A panel still in the holder is
    // one no column is showing, and an invisible parent is what keeps it from
    // painting itself over the corner of the window.
    Item {
        id: holder
        visible: false

        LeftPanel { id: leftPanel }
        CenterPanel { id: centerPanel }
        TerminalPanel { id: terminalPanel }
        FilesPanel { id: filesPanel }
        BrowserPanel { id: browserPanel }
        DiffPanel { id: diffPanel }
        GitPanel { id: gitPanel }
    }

    Dock {
        id: dock
        anchors.fill: parent
        panels: ({
            "left": leftPanel,
            "center": centerPanel,
            "right": terminalPanel,
            "files": filesPanel,
            "browser": browserPanel,
            "diff": diffPanel,
            "git": gitPanel
        })
        titles: ({
            "left": qsTr("History"),
            "center": qsTr("Conversation"),
            "right": qsTr("Terminal"),
            "files": qsTr("Files"),
            "browser": qsTr("Browser"),
            "diff": qsTr("Changes"),
            "git": qsTr("Git")
        })
    }

    // A link in a conversation goes to the browser pane, which is shown if it
    // was not. The transcript raises the request and knows nothing about who
    // answers it; this is the only place that knows both panels exist.
    Connections {
        target: Session
        function onLink_activated(url) {
            App.set_panel_visible("browser", true)
            Browser.open_url(url)
        }
    }

    // Last tab closed. An empty browser pane is a stripe of nothing rather than
    // a browser, so the panel goes with it — and the side strip's toggle goes
    // out, because it reads the same panel state.
    Connections {
        target: Browser
        function onEmptied() { App.set_panel_visible("browser", false) }
    }

    // Signing in to a ChatGPT plan. pi's OAuth flow lives only in its own
    // interactive UI — there is no RPC command and no headless CLI for it — so
    // the honest thing is to start the flow where it works and get out of the
    // way: a terminal in this workspace, running pi, with the command to type
    // printed above it.
    Connections {
        target: Settings
        function onCodex_signin_requested() { view.startCodexSignin() }
    }

    function startCodexSignin() {
        // The terminal is asked for before the panel is shown, not after:
        // showing it builds the pane, and a pane builds itself with a first tab
        // already open. Asked afterwards, this would hand back a *second*
        // terminal and leave that first shell idle behind the sign-in.
        App.set_panel_visible("right", true)
        signinDelay.restart()
    }

    Timer {
        id: signinDelay
        // Let the shell reach its prompt first. The bytes would survive in the
        // pty either way, but typed before the prompt they land above it and
        // read as a terminal that has gone wrong.
        interval: 500
        onTriggered: TerminalTabs.paste(
            "echo 'In pi, run: /login openai-codex'\npi\n")
    }

    // A capture of the page, attached to the next prompt.
    Connections {
        target: browserPanel
        function onCaptured(path) { Session.attach_file(path) }
    }

    // The side strip and the title bar toggle panels through App; the dock is
    // what actually shows and hides them.
    Connections {
        target: App
        function onPanels_changed() { view.syncPanels() }
        function onCurrent_changed() {
            view.syncPanels()
            // A workspace can be restored with more panels than this window has
            // room for, so its saved layout is checked the way a resize is.
            fitPanels.restart()
        }
    }

    // History lists both what pi has written and what the workspace is holding
    // right now, and neither bridge knows the other exists — this is the seam
    // that puts them together. Without it a turn that is streaming before pi
    // has named its file has no row, so clicking away from it strands it.
    Connections {
        target: Session
        function onLive_changed() { view.syncSessions() }
    }

    function syncSessions() {
        History.set_live_sessions(JSON.stringify(Session.live_sessions))
        History.set_selected_key(Session.focused_key)
    }

    // The side strip's camera, routed to the panel that has a page to capture.
    function capturePage() {
        browserPanel.capture()
    }

    function syncPanels() {
        for (const side of ["left", "files", "browser", "diff", "git", "right"])
            DockModel.set_panel_visible(side, App.is_panel_visible(side))
    }

    // Narrow the window far enough and there is no arrangement that fits:
    // History is a fixed width, the conversation has a floor, and the side
    // panels are already at theirs. Rather than lay the rest out past the right
    // edge — where they can be neither seen nor reached — the ones furthest
    // from the conversation are closed, exactly as if their toggle had been
    // clicked. The side strip reads the same state, so it goes out with them
    // and says what is actually open.
    //
    // They stay closed when the window is widened again. Re-opening on its own
    // would mean holding a second, invisible idea of which panels are "really"
    // open and overruling the toggles with it.
    function closeWhatDoesNotFit() {
        // Before the first layout this is a width no panel could fit in — the
        // anchors have been applied to nothing yet — and acting on it would
        // close every side panel at startup, whatever size the window is.
        if (view.width <= 0)
            return
        const closing = JSON.parse(DockModel.panels_that_do_not_fit(view.width))
        for (const key of closing)
            App.set_panel_visible(key, false)
    }

    // Only a resize and a workspace switch trigger this, never a panel toggle:
    // at capacity the rule keeps the panels nearest the conversation, so running
    // it on a toggle would close whichever panel had just been opened and read
    // as the toggle doing nothing at all.
    onWidthChanged: fitPanels.restart()

    // A drag on the window edge resizes every frame; only the width it comes to
    // rest at is worth closing a panel over.
    Timer {
        id: fitPanels
        interval: 120
        onTriggered: view.closeWhatDoesNotFit()
    }

    Component.onCompleted: {
        syncPanels()
        syncSessions()
    }
}
