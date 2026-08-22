import QtQuick
import QtQuick.Controls
import "../common"
import "../chat"

// The conversation pane: the transcript, the busy row, and the composer.
//
// Content is capped at a reading width and centred. The transcript is the full
// width of the panel with its column centred inside it, and the two rows below
// are centred on the same panel — so all three line up without anybody having
// to reserve a gutter, and the scrollbar overlays the panel's right edge
// instead of taking a strip out of the layout. (The widget port had to reserve
// that strip by hand on both rows, and got it four pixels wrong.)
Rectangle {
    id: panel

    color: Theme.colors.card

    // The reading column. Prose much wider than this is measurably harder to
    // follow, and a docked panel can be far wider.
    readonly property int maxContentWidth: 1000
    readonly property int contentWidth: Math.min(width - 24, maxContentWidth)

    // The clock the whole pane runs on. The bridge has no event loop of its
    // own by design, so this is what drains the agents and decides when a
    // streaming transcript is worth repainting; the cadence inside the bridge
    // is what keeps that from being once per token.
    Timer {
        interval: 50
        repeat: true
        running: true
        onTriggered: Session.pump()
    }

    Column {
        anchors.fill: parent
        spacing: 0

        Transcript {
            id: transcript
            width: parent.width
            height: parent.height - busyRow.height - composer.height - 12
            blocks: Session.blocks
            columnWidth: panel.maxContentWidth

            ScrollBar.vertical: ThinScrollBar {}

            onToggleRequested: (index, open) => Session.set_expanded(index, open)
            onCopyRequested: (source) => clipboard.copy(Session.copy_code(source))
            onLinkActivated: (url) => Session.open_link(url)
        }

        // The turn's own status line: what pi is doing, how long it has been
        // doing it, how full the context is, and the way out.
        Item {
            id: busyRow
            width: parent.width
            height: visible ? label.implicitHeight + 6 : 0
            visible: Session.busy || Session.compacting

            Row {
                anchors {
                    left: parent.left
                    leftMargin: Math.max(4, (panel.width - panel.contentWidth) / 2 + 4)
                    verticalCenter: parent.verticalCenter
                }
                spacing: 8

                Text {
                    id: label
                    // Stop-armed outranks compacting: it is the only one of the
                    // three the reader has to answer, and a wedged turn is not
                    // made less wedged by what pi was doing when it wedged.
                    text: Session.stop_armed ? qsTr("Pi isn't responding to Stop…")
                        : Session.compacting ? qsTr("Pi is compacting context…")
                        : qsTr("Pi is working…")
                    color: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    font.italic: true
                }

                Text {
                    text: busyRow.elapsedLabel
                    color: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    font.italic: true
                }
            }

            Row {
                anchors {
                    right: parent.right
                    rightMargin: Math.max(4, (panel.width - panel.contentWidth) / 2 + 4)
                    verticalCenter: parent.verticalCenter
                }
                spacing: 8

                Text {
                    anchors.verticalCenter: parent.verticalCenter
                    visible: Session.context_label !== ""
                    text: Session.context_label
                    color: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    font.italic: true
                }

                TextButton {
                    anchors.verticalCenter: parent.verticalCenter
                    text: Session.stop_armed ? qsTr("Force stop") : qsTr("Stop")
                    onClicked: Session.stop()
                }
            }

            // The elapsed count is derived rather than delivered: the bridge
            // publishes when the turn started, and a property that carried the
            // seconds instead would have to notify once a second for a number
            // that can be worked out here.
            property real now: Date.now()
            readonly property string elapsedLabel:
                Session.busy_since <= 0 ? ""
                                        : format(Math.floor((now - Session.busy_since) / 1000))

            function format(seconds) {
                seconds = Math.max(seconds, 0)
                if (seconds < 60)
                    return seconds + "s"
                const minutes = Math.floor(seconds / 60)
                const rest = ("0" + (seconds % 60)).slice(-2)
                if (minutes < 60)
                    return minutes + ":" + rest
                return Math.floor(minutes / 60) + ":"
                       + ("0" + (minutes % 60)).slice(-2) + ":" + rest
            }

            Timer {
                interval: 1000
                repeat: true
                running: busyRow.visible
                onTriggered: busyRow.now = Date.now()
            }
        }

        ChatInput {
            id: composer
            x: Math.max(0, (panel.width - panel.contentWidth) / 2)
            width: panel.contentWidth
            busy: Session.busy
            stopArmed: Session.stop_armed
            onSubmitted: (text, files) => Session.submit(text, files)
        }

        // Keeps the rounded composer clear of the panel edge. The gap lives out
        // here rather than inside the composer, so its own padding stays
        // balanced within its border.
        Item { width: 1; height: 12 }
    }

    // Nothing said yet: name the session rather than leaving a blank panel,
    // which reads as a pane that failed to load. Overlaid rather than parented
    // into the transcript — a Flickable's children ride its content item, which
    // is a few pixels tall when there is no content to centre in.
    Text {
        x: transcript.x + (transcript.width - width) / 2
        y: transcript.y + (transcript.height - height) / 2
        visible: transcript.blocks.length === 0
        text: qsTr("Ask Pi anything about this workspace.")
        color: Theme.chat_colors.dim
        font.family: Theme.mono_family
        font.pixelSize: Theme.chat_font_size
    }

    Clipboard { id: clipboard }

    // ---- Wiring ---------------------------------------------------------------

    // Pushed in rather than read out of Theme by the bridge: the bridge renders
    // the transcript's markup and needs the colours and the base size, but it
    // has one owner and no opinion about which other objects exist.
    Connections {
        target: Theme
        function onChanged() {
            Session.set_theme(Theme.name)
            Session.set_font_size(Theme.chat_font_size)
        }
    }

    Connections {
        target: App
        function onCurrent_changed() { Session.set_workspace(App.current) }
    }

    // The title bar shows what the focused conversation is called.
    Connections {
        target: Session
        function onTitle_changed() { App.set_conversation(Session.title) }
    }

    Connections {
        target: History
        function onSession_activated(key) {
            // A row for a session the workspace is already holding is a focus,
            // not a load: its agent is running and its transcript is in memory,
            // and re-reading the file would throw both away.
            const live = Session.live_sessions.find((entry) => entry.key === key)
            if (live) {
                Session.focus_session(key)
                return
            }
            // A persisted row's key is its file path.
            const row = History.sessions.find((entry) => entry.key === key)
            if (row && !row.live)
                Session.load_session(key)
        }
        function onNew_session_requested() { Session.new_session() }
        // Archived or deleted. If it is the one on screen, there is nothing
        // left to look at, so the pane opens a fresh session rather than
        // showing a transcript whose file no longer exists.
        function onSession_removed(path) {
            const live = Session.live_sessions.find((entry) => entry.path === path)
            if (live && live.current)
                Session.new_session()
        }
    }

    Component.onCompleted: {
        Session.set_theme(Theme.name)
        Session.set_font_size(Theme.chat_font_size)
        Session.set_workspace(App.current)
    }
}
