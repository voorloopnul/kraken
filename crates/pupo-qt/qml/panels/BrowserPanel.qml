import QtQuick
import QtQuick.Controls
import "../common"
import "../settings"

// The browser pane: a tab strip in the panel's header, a nav row, and the page.
//
// The renderer is loaded at run time rather than imported, because QtWebEngine
// is a separate package that may simply not be installed — and an `import` of a
// module that is not there is not a missing feature, it is a QML file that fails
// to load and takes the panel with it. So the view is built with
// `Qt.createQmlObject` inside a try, and when that fails the pane says so and
// everything except the page keeps working: the tabs, the address bar, the
// history of what was opened.
Item {
    id: panel

    property Item tabStrip: strip

    // Whether this machine can draw a page at all. Probed once, on the first
    // sight of the panel — the answer cannot change while the app is running.
    property bool engineReady: false
    property bool probed: false

    Item {
        id: stripHolder
        visible: false

        TabStrip {
            id: strip
            tabs: Browser.tabs
            current: Browser.current
            onSelected: (id) => Browser.select_tab(id)
            onClosed: (id) => Browser.close_tab(id)
            onAdded: Browser.add_tab()
            onMoved: (from, to) => Browser.move_tab(from, to)
        }
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.colors.card

        // ---- Address row -----------------------------------------------------

        Item {
            id: nav
            anchors { left: parent.left; right: parent.right; top: parent.top }
            anchors.margins: 6
            height: 26

            Row {
                id: back
                anchors { left: parent.left; verticalCenter: parent.verticalCenter }
                spacing: 2

                IconButton {
                    implicitWidth: 22
                    implicitHeight: 22
                    glyphSize: 13
                    glyph: "arrow-left"
                    tooltip: qsTr("Back")
                    onClicked: panel.callPage("goBack")
                }
                IconButton {
                    implicitWidth: 22
                    implicitHeight: 22
                    glyphSize: 13
                    glyph: "arrow-left"
                    rotation: 180
                    tooltip: qsTr("Forward")
                    onClicked: panel.callPage("goForward")
                }
                TextButton {
                    text: "↻"
                    fontSize: 13
                    tooltip: qsTr("Reload")
                    onClicked: panel.callPage("reload")
                }
            }

            SettingsField {
                id: address
                anchors {
                    left: back.right; leftMargin: 6
                    right: parent.right
                    verticalCenter: parent.verticalCenter
                }
                width: undefined
                placeholderText: qsTr("Search, or type a URL")
                text: Browser.url
                onAccepted: Browser.open_url(text)
            }
        }

        // ---- Page ------------------------------------------------------------

        Item {
            id: stage
            anchors {
                left: parent.left; right: parent.right
                top: nav.bottom; bottom: parent.bottom
                margins: 6
                topMargin: 0
            }
            clip: true

            // Where the renderer goes when there is one. Empty otherwise, with
            // one of the notices below over it.
            Item {
                id: viewport
                anchors.fill: parent
            }

            // The honest surface when QtWebEngine is not installed. Named, so
            // the reader knows what to install rather than that something is
            // wrong.
            Column {
                anchors.centerIn: parent
                width: Math.min(parent.width - 48, 420)
                spacing: 10
                visible: panel.probed && !panel.engineReady

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    text: qsTr("No browser engine")
                    color: Theme.colors.text
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.chat_font_size
                    font.weight: Font.DemiBold
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: qsTr("Pupo draws pages with QtWebEngine, which is not "
                               + "installed on this machine. Install "
                               + "qml6-module-qtwebengine (qt6-webengine on some "
                               + "distributions) and restart Pupo.\n\n"
                               + "Everything else in this panel works: tabs, the "
                               + "address bar, and links opened from a conversation.")
                    color: Theme.chat_colors.dim
                    font.family: Theme.sans_family
                    font.pixelSize: 12
                    lineHeight: 1.3
                }

                Text {
                    width: parent.width
                    horizontalAlignment: Text.AlignHCenter
                    visible: Browser.url !== ""
                    text: qsTr("This tab: %1").arg(Browser.url)
                    color: Theme.chat_colors.dim
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.caption_font_size
                    elide: Text.ElideMiddle
                }
            }

            // A page whose renderer died. Reloading is offered rather than done:
            // a page that crashes on load crashes again on reload, and doing it
            // automatically is a loop.
            Column {
                anchors.centerIn: parent
                spacing: 10
                visible: panel.engineReady && Browser.crashed

                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: qsTr("This page stopped responding.")
                    color: Theme.colors.text
                    font.family: Theme.sans_family
                    font.pixelSize: 13
                }
                SettingsChip {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: qsTr("Reload")
                    onClicked: panel.callPage("reload")
                }
            }
        }
    }

    // ---- The renderer ---------------------------------------------------------

    // One view for the whole panel rather than one per tab, and the tab's URL
    // pushed into it on every switch.
    //
    // Chromium keeps a renderer process per live view: a view per tab is a
    // process per tab, all of them resident whether or not anyone is looking.
    // The cost of the other way round is that a background tab does not keep
    // loading — which for a panel beside a conversation is the right trade.
    property var page: null

    function probe() {
        if (probed)
            return
        probed = true
        engineReady = createView()
        if (engineReady)
            show(Browser.url)
    }

    // Build the view and wire it up. Answers whether it could be built at all,
    // which on a machine without the QtWebEngine module it cannot.
    function createView() {
        try {
            page = Qt.createQmlObject(
                'import QtQuick; import QtWebEngine;'
                + ' WebEngineView { anchors.fill: parent }',
                viewport, "BrowserPanel.renderer")
        } catch (error) {
            // Expected on a machine without the module; the notice above says
            // so, and everything else in the panel goes on working.
            page = null
            return false
        }
        page.urlChanged.connect(report)
        page.titleChanged.connect(report)
        page.renderProcessTerminated.connect(function () {
            Browser.page_crashed(Browser.current)
        })
        return true
    }

    // Tear the renderer down when the pane has no tabs left.
    //
    // Hiding the panel is not enough, and neither is closing the tab: the view
    // used to be built once and kept for the life of the app, so Chromium held
    // the page — and the processes drawing it — long after the browser had been
    // closed. A browser nobody has open should not still cost half a gigabyte.
    //
    // `engineReady` is deliberately left alone: whether the module is installed
    // was answered once by `probe` and cannot change while the app runs. Only
    // the view goes, and `show` builds another when a page is next wanted.
    function discard() {
        if (!page)
            return
        // Leave the page before destroying the view: that stops the loading,
        // the timers and the media a live page still has running.
        page.url = "about:blank"
        page.destroy()
        page = null
    }

    function report() {
        if (page)
            Browser.page_changed(Browser.current, page.url.toString(), page.title)
    }

    // Point the page at a URL — but only if it is not already there.
    //
    // Assigning `page.url` is a *navigation*, not a label, and this is reached
    // from `onTabs_changed`, which the bridge emits every time the page reports
    // where it went. Without the guard those two chase each other: the page
    // announces a URL, the bridge signals, this reloads the page at the URL it
    // is already on, which announces again. A static page settles because the
    // second assignment is the same string; one whose script rewrites its own
    // URL — an ad, a redirect, any `history.pushState` — never does, and the
    // panel reloads it as fast as it can render, which reads as a frantic
    // flicker with the memory climbing behind it.
    function show(url) {
        // Rebuilt on demand: the view is destroyed whenever the pane empties,
        // so wanting a page again is what brings one back.
        if (!page) {
            if (!engineReady || !createView())
                return
        }
        const wanted = url === "" ? "about:blank" : url
        if (page.url.toString() === wanted)
            return
        page.url = wanted
    }

    // A picture of the page, for the conversation to look at. Saved to a file
    // rather than handed over as data: the composer attaches paths, and a PNG
    // of a browser window as a base64 property is a megabyte QML would re-parse
    // on every read.
    function capture() {
        if (!page)
            return ""
        const path = Browser.capture_path()
        if (path === "")
            return ""
        const started = page.grabToImage(function (result) {
            if (result.saveToFile(path))
                panel.captured(path)
        })
        return started ? path : ""
    }

    signal captured(string path)

    function callPage(what) {
        if (!page)
            return
        if (what === "goBack") page.goBack()
        else if (what === "goForward") page.goForward()
        else page.reload()
    }

    // ---- Wiring ---------------------------------------------------------------

    onVisibleChanged: if (visible) start()

    function start() {
        if (!visible)
            return
        probe()
        Browser.ensure_started()
    }

    Connections {
        target: Browser
        // The tab in front changed, or something asked for a page.
        function onTabs_changed() {
            // No tabs left: the browser is closed, whatever the panel is doing.
            if (!Browser.started)
                panel.discard()
            else
                panel.show(Browser.url)
        }
        function onLoad_requested(id, url) { panel.show(url) }
    }

    Connections {
        target: App
        function onCurrent_changed() { Browser.set_workspace(App.current) }
    }

    Component.onCompleted: {
        Browser.set_workspace(App.current)
        start()
    }
}
