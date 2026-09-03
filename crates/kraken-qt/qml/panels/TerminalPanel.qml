import QtQuick
import QtQuick.Controls
import "../common"

// The terminal: a tab strip in the panel's header, and the grid under it.
//
// Everything that *is* a terminal — the pty, the VT engine, key encoding,
// selection, scrollback — lives in `kraken_core::terminal`, and the bridge hands
// this a finished frame. So there is no terminal logic here at all: this
// measures the cell, paints what it is given, and hands input back.
//
// One `Text` per row rather than one per styled run. The bridge replaces the
// whole frame on every repaint, and a `Repeater` over the runs would destroy and
// rebuild every span in the grid sixty times a second; a repeater over a row
// *count* keeps its delegates and only re-evaluates a string, which for an
// unchanged row costs nothing at all.
Item {
    id: panel

    // Mounted into the dock's panel header; see DockPanel.qml.
    property Item tabStrip: strip

    readonly property real cellWidth: probe.implicitWidth / 10
    readonly property real cellHeight: probe.implicitHeight

    // The cell, measured off a rendered line rather than asked of FontMetrics.
    //
    // Two reasons, and the second is the one that bites: `advanceWidth` is a
    // *method*, so a binding on it records no dependency and keeps the width of
    // whatever fallback face was loaded when the tree was built — the bundled
    // font arrives later and the grid never hears about it. Measuring an item's
    // implicit size is a property read, so it settles when the font does. It is
    // also the honest number: this is the width the glyphs actually came out at,
    // and it is what the shell has to be told.
    Text {
        id: probe
        visible: false
        text: "MMMMMMMMMM"
        font.family: Theme.mono_family
        // Points, not pixels: the terminal's size is a point size everywhere
        // else in the app and in `~/.kraken/state.json`.
        font.pointSize: Theme.terminal_font_size
    }

    // Built here so it exists while the panel is parked in the holder, and
    // reparented into the header by the dock. An Item with no parent is never
    // laid out, so it has to have one it can be taken from.
    Item {
        id: stripHolder
        visible: false

        TabStrip {
            id: strip
            tabs: TerminalTabs.tabs
            current: TerminalTabs.current
            onSelected: (id) => TerminalTabs.select_tab(id)
            onClosed: (id) => TerminalTabs.close_tab(id)
            onAdded: TerminalTabs.add_tab()
            onMoved: (from, to) => TerminalTabs.move_tab(from, to)
        }
    }

    Rectangle {
        id: surface
        anchors.fill: parent
        // Keys land on the same item the mouse does, so a click in the grid is
        // what takes them and clicking the tab strip up in the header never
        // steals them from the shell.
        focus: true
        // The terminal's own background, not the card's — they are the same
        // colour by design (see UI_COLORS), and reading it from here is what
        // keeps them the same when a theme sets its own.
        color: TerminalTabs.background

        // Cells are laid out from a fractional cell width rather than a rounded
        // one: the error in a rounded width accumulates across eighty columns
        // into a visible drift between the text and the cursor over it.
        readonly property real cw: panel.cellWidth
        readonly property real ch: panel.cellHeight

        Item {
            id: grid
            anchors.fill: parent
            clip: true

            // A row count rather than the rows themselves: the count changes on
            // a resize, the contents change on every frame, and only the first
            // of those should cost a rebuild.
            Repeater {
                model: panel.frameRows.length

                Item {
                    id: line
                    required property int index

                    y: index * surface.ch
                    width: grid.width
                    height: surface.ch

                    // Cell backgrounds, under the text. Only runs that differ
                    // from the screen's own background are painted — the rest
                    // is the surface already showing through.
                    Repeater {
                        model: panel.backgroundsFor(line.index)

                        Rectangle {
                            required property var modelData
                            x: modelData.col * surface.cw
                            width: modelData.cols * surface.cw
                            height: surface.ch
                            color: modelData.bg
                        }
                    }

                    // Above the cell backgrounds and below the text, which is
                    // what keeps selected text readable rather than washed out.
                    Rectangle {
                        readonly property var span: panel.selectionFor(line.index)
                        visible: span.cols > 0
                        x: span.col * surface.cw
                        width: span.cols * surface.cw
                        height: surface.ch
                        color: Qt.rgba(120 / 255, 150 / 255, 210 / 255, 110 / 255)
                    }

                    Text {
                        width: parent.width
                        height: parent.height
                        textFormat: Text.RichText
                        text: panel.markupFor(line.index)
                        color: TerminalTabs.foreground
                        font: probe.font
                        // Kept off so a long line runs past the edge and is
                        // clipped, the way a terminal's own line is.
                        wrapMode: Text.NoWrap
                    }
                }
            }

            // The cursor, over everything. A filled block hides the glyph under
            // it, so the character it covers is redrawn on top in the
            // background colour — which is why the bridge reports it.
            Item {
                id: cursor
                visible: TerminalTabs.cursor_style !== ""
                x: TerminalTabs.cursor_col * surface.cw
                y: TerminalTabs.cursor_row * surface.ch
                width: surface.cw
                height: surface.ch

                Rectangle {
                    anchors.fill: parent
                    visible: TerminalTabs.cursor_style === "block"
                    color: TerminalTabs.foreground
                }

                Rectangle {
                    anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
                    width: 2
                    visible: TerminalTabs.cursor_style === "bar"
                    color: TerminalTabs.foreground
                }

                Rectangle {
                    anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                    height: 2
                    visible: TerminalTabs.cursor_style === "underline"
                    color: TerminalTabs.foreground
                }

                // The unfocused cursor: an outline, so two panes side by side
                // never both look like the one being typed into.
                Rectangle {
                    anchors.fill: parent
                    visible: TerminalTabs.cursor_style === "hollow"
                    color: "transparent"
                    border.width: 1
                    border.color: TerminalTabs.foreground
                }

                Text {
                    anchors.fill: parent
                    visible: TerminalTabs.cursor_style === "block"
                    text: TerminalTabs.cursor_text
                    color: TerminalTabs.background
                    font: probe.font
                }
            }
        }

        // Nothing spawned yet: this workspace's shell is not started until the
        // panel is first shown, so a project whose terminal you never open costs
        // no shell at all.
        Text {
            anchors.centerIn: parent
            visible: !TerminalTabs.started
            text: qsTr("Starting a shell…")
            color: Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: Theme.chat_font_size
        }

        // ---- Input -----------------------------------------------------------

        MouseArea {
            id: mouse
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.IBeamCursor
            acceptedButtons: Qt.LeftButton | Qt.MiddleButton | Qt.RightButton

            // How many clicks the current press is part of. Qt gives QML a
            // doubleClicked but no tripleClicked, so the run is counted here.
            property int clicks: 0
            property real lastAt: 0

            function cell(x, y) {
                return {
                    col: Math.max(0, Math.floor(x / surface.cw)),
                    row: Math.max(0, Math.floor(y / surface.ch))
                }
            }

            onPressed: function (event) {
                surface.forceActiveFocus()
                if (event.button === Qt.RightButton) {
                    // The selection is left exactly as it was: the menu's own
                    // first item is about to act on it.
                    menu.openHere()
                    return
                }
                if (event.button === Qt.MiddleButton) {
                    // X11's other clipboard. Qt exposes it to QML only through a
                    // TextEdit's paste, which has no selection buffer of its
                    // own, so the terminal pastes its own selection — the
                    // common case, and the only one reachable from here.
                    TerminalTabs.paste(TerminalTabs.copy_selection())
                    return
                }
                const now = Date.now()
                clicks = (now - lastAt < 400) ? clicks + 1 : 1
                lastAt = now
                const at = cell(event.x, event.y)
                const mode = clicks >= 3 ? "line" : clicks === 2 ? "word" : "character"
                TerminalTabs.select_start(at.col, at.row, mode)
            }

            onPositionChanged: function (event) {
                // The left button specifically: a menu that came up under the
                // right one takes the grab and gives no release back, and a
                // plain `pressed` would have the pointer dragging a selection
                // around long after the click that opened it.
                if (!(pressedButtons & Qt.LeftButton))
                    return
                const at = cell(event.x, event.y)
                TerminalTabs.select_extend(at.col, at.row)
                // A drag past either edge keeps going, so a selection can run
                // off the top of the viewport into the scrollback.
                autoscroll.lines = event.y < 0 ? -1 : event.y > height ? 1 : 0
            }

            onReleased: function (event) {
                if (event.button !== Qt.LeftButton)
                    return
                autoscroll.lines = 0
                // Straight to the clipboard, the way every terminal does it:
                // the selection is the copy, and a second gesture to confirm it
                // is one nobody makes.
                const text = TerminalTabs.copy_selection()
                if (text !== "")
                    clipboard.copy(text)
            }

            onWheel: function (event) {
                TerminalTabs.wheel(event.angleDelta.y)
            }
        }

        // Right-click: the three things a terminal is asked for by pointer.
        //
        // What each item can do is read when the menu opens rather than bound
        // to it — both answers come from methods on the bridge, and a binding
        // over a method call records no dependency and would keep whatever was
        // true the first time the menu was built.
        Menu {
            id: menu

            property bool hasSelection: false
            property bool canClear: false

            function openHere() {
                hasSelection = TerminalTabs.has_selection()
                canClear = !TerminalTabs.on_alt_screen()
                popup()
            }

            // The menu takes the focus while it is up, and the grid is what
            // keys belong to.
            onClosed: surface.forceActiveFocus()

            MenuItem {
                text: qsTr("Copy")
                enabled: menu.hasSelection
                onTriggered: clipboard.copy(TerminalTabs.copy_selection())
            }
            MenuItem {
                text: qsTr("Paste")
                onTriggered: TerminalTabs.paste(clipboard.paste())
            }
            MenuSeparator {}
            MenuItem {
                text: qsTr("Clear")
                // Off while a full-screen program owns the screen: what is
                // drawn there is vim's, and vim will not know to redraw it.
                enabled: menu.canClear
                onTriggered: TerminalTabs.clear()
            }
        }

        Timer {
            id: autoscroll
            property int lines: 0
            interval: 50
            repeat: true
            running: lines !== 0
            onTriggered: TerminalTabs.scroll_lines(lines)
        }

        Keys.onPressed: function (event) {
            // Copy and paste are the two chords the shell never gets to see:
            // Ctrl+C is an interrupt down there, so the copy has to be taken
            // above it — which is why both of them carry Shift.
            if ((event.modifiers & Qt.ControlModifier) && (event.modifiers & Qt.ShiftModifier)) {
                if (event.key === Qt.Key_C) {
                    clipboard.copy(TerminalTabs.copy_selection())
                    event.accepted = true
                    return
                }
                if (event.key === Qt.Key_V) {
                    TerminalTabs.paste(clipboard.paste())
                    event.accepted = true
                    return
                }
            }
            TerminalTabs.key(event.key, event.modifiers, event.text)
            event.accepted = true
        }

        onActiveFocusChanged: TerminalTabs.set_focused(activeFocus)
    }

    Clipboard { id: clipboard }

    // ---- Frame reading -------------------------------------------------------

    // The frame's rows, read from the bridge once and shared by the three
    // helpers below.
    //
    // `TerminalTabs.rows` is built on every read — the bridge converts the whole
    // grid into lists of maps each time it is asked — and the helpers run once
    // per row apiece, so reading it in each of them rebuilt the grid three times
    // per row. Bound here it is rebuilt once per frame, and the binding still
    // invalidates on exactly the same signal, so every row repaints when it did
    // before.
    readonly property var frameRows: TerminalTabs.rows

    // Row markup, built from the runs the engine already collapsed.
    //
    // Spaces become `&nbsp;`: rich text folds runs of whitespace away, and a
    // terminal's alignment is entirely made of them. In a monospace face the two
    // have the same advance, so the grid is unaffected.
    function markupFor(index) {
        const row = panel.frameRows[index]
        if (!row)
            return ""
        let out = ""
        let col = 0
        for (const run of row.runs) {
            // Runs are contiguous in the engine, but a gap costs nothing to
            // handle and a misaligned line costs the whole grid.
            if (run.col > col)
                out += "&nbsp;".repeat(run.col - col)
            col = run.col + run.cols
            let text = run.text
                .replace(/&/g, "&amp;")
                .replace(/</g, "&lt;")
                .replace(/>/g, "&gt;")
                .replace(/ /g, "&nbsp;")
            if (run.bold) text = "<b>" + text + "</b>"
            if (run.italic) text = "<i>" + text + "</i>"
            if (run.underline) text = "<u>" + text + "</u>"
            if (run.strike) text = "<s>" + text + "</s>"
            out += "<span style=\"color:" + run.fg + ";\">" + text + "</span>"
        }
        return out
    }

    // The runs whose background differs from the screen's, as `{col, cols, bg}`.
    function backgroundsFor(index) {
        const row = panel.frameRows[index]
        if (!row)
            return []
        const base = TerminalTabs.background.toLowerCase()
        const out = []
        for (const run of row.runs) {
            if (run.bg.toLowerCase() === base)
                continue
            out.push({ col: run.col, cols: run.cols, bg: run.bg })
        }
        return out
    }

    // The selected span of one row, as `{col, cols}`; `cols` is 0 for none. The
    // bridge reports an inclusive range, which is one cell wider than it counts.
    function selectionFor(index) {
        const row = panel.frameRows[index]
        if (!row || row.sel_start < 0)
            return { col: 0, cols: 0 }
        return { col: row.sel_start, cols: row.sel_end - row.sel_start + 1 }
    }

    // ---- Wiring --------------------------------------------------------------

    // The grid is measured from the font that was actually rendered, and told to
    // the shell: only this side knows what the glyphs came out as, and a shell
    // told a size that does not match what is drawn gets every full-screen
    // program wrong.
    // Answers whether a size was actually applied: until the font has loaded
    // and the panel has been laid out there is no honest grid to send, and a
    // shell must not be born against one that is missing.
    function applySize() {
        if (panel.cellWidth <= 0 || panel.cellHeight <= 0 || width <= 0 || height <= 0)
            return false
        TerminalTabs.resize(Math.floor(width / panel.cellWidth),
                            Math.floor(height / panel.cellHeight),
                            Math.round(panel.cellWidth),
                            Math.round(panel.cellHeight))
        return true
    }

    onWidthChanged: resizeSoon.restart()
    onHeightChanged: resizeSoon.restart()
    onCellWidthChanged: resizeSoon.restart()
    onCellHeightChanged: resizeSoon.restart()

    // A drag across the dock resizes the panel every frame, and each size is a
    // SIGWINCH the shell has to answer. Only the size it lands on is worth
    // sending.
    Timer {
        id: resizeSoon
        interval: 60
        onTriggered: {
            const sized = panel.applySize()
            // The shell waits for the same quiet the resize does. Spawned
            // before the dock has finished laying the new workspace out, it
            // prints its prompt at one width and is then sent a SIGWINCH for
            // another — and bash redraws the prompt over the one already
            // there, which reads as the text having duplicated itself.
            if (sized && panel.pendingStart && panel.visible) {
                panel.pendingStart = false
                TerminalTabs.ensure_started()
            }
        }
    }



    Timer {
        interval: 16
        repeat: true
        running: true
        onTriggered: TerminalTabs.pump()
    }

    Connections {
        target: Theme
        function onChanged() {
            TerminalTabs.set_theme(Theme.name)
            TerminalTabs.set_font_size(Theme.terminal_font_size)
        }
    }

    Connections {
        target: App
        function onCurrent_changed() {
            TerminalTabs.set_workspace(App.current)
            // Switching workspace does not make an already-open panel newly
            // visible, so `onVisibleChanged` never fires and the shell this
            // workspace is supposed to start is never spawned — the panel sits
            // on "Starting a shell…" with an empty tab strip behind it.
            // Deferred: `WorkspaceView` re-syncs which panels the dock shows
            // on this same signal, and the order of two handlers on one signal
            // is not ours to choose. Called straight away this can run while
            // the panel is still parked and invisible, `start()` bails on that,
            // and the shell is never spawned.
            Qt.callLater(start)
        }
    }

    // Spawned on first sight, not on startup: a workspace whose terminal you
    // never open costs no shell, no reader thread and no scrollback.
    onVisibleChanged: if (visible) start()

    // A shell is born knowing the width it will print into, so the spawn is
    // queued behind the same debounce a resize is: every width change restarts
    // it, and what starts the shell is the panel going quiet.
    property bool pendingStart: false

    function start() {
        if (!visible)
            return
        pendingStart = true
        resizeSoon.restart()
    }

    Component.onCompleted: {
        TerminalTabs.set_theme(Theme.name)
        TerminalTabs.set_font_size(Theme.terminal_font_size)
        TerminalTabs.set_workspace(App.current)
        start()
    }
}
