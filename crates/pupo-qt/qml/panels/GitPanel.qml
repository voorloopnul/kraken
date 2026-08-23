import QtQuick
import "../common"

// The Git pane: what has changed, and what has been committed.
//
// Two views of one repository, so one panel with two tabs rather than two
// panels side by side. They answer the same question a minute apart — what the
// agent has been doing, and what of it has landed — and a workspace holding
// both open paid a whole column for the half nobody was reading.
//
// The views are built once and switched by visibility, which is also what makes
// them refresh: each re-reads git when it comes into view, so the one behind
// runs no subprocess and the one in front is never stale. ChangesView and
// CommitsView are the panes themselves; this file only chooses between them.
Item {
    id: panel

    // Both mount into the dock's panel header rather than sitting under it; see
    // DockPanel.qml. The strip is the panel's title as well as its switch.
    property Item tabStrip: strip
    property Item headerTools: tools

    // Which view is up, by tab id.
    property int current: 0

    readonly property int changesTab: 0
    readonly property int commitsTab: 1

    Item {
        id: chrome
        visible: false

        TabStrip {
            id: strip
            // Two views the panel has always had, not tabs anyone opened: there
            // is nothing to close, nothing to add and no order to put them in.
            fixed: true
            tabs: [
                { id: panel.changesTab, title: qsTr("Changes") },
                { id: panel.commitsTab, title: qsTr("Commits") }
            ]
            current: panel.current
            onSelected: function (id) { panel.current = id }
        }

        // One Refresh for both views, because there is one repository behind
        // them; which of its two questions to ask again is whichever view is up.
        //
        // A character rather than an icon: the vendored Lucide set has no
        // reload glyph, and a rotated arrow from it points somewhere and so
        // says something else.
        TextButton {
            id: tools
            text: "↻"
            fontSize: 14
            tooltip: qsTr("Refresh")
            onClicked: {
                if (panel.current === panel.changesTab)
                    Diff.refresh()
                else
                    Git.refresh()
            }
        }
    }

    ChangesView {
        anchors.fill: parent
        visible: panel.current === panel.changesTab
    }

    CommitsView {
        anchors.fill: parent
        visible: panel.current === panel.commitsTab
    }

    // ---- Wiring ---------------------------------------------------------------

    // Both bridges read the same repository, so they are pointed at it once
    // here rather than twice over in the views.
    Binding { target: Diff; property: "workspace"; value: App.current }
    Binding { target: Git; property: "workspace"; value: App.current }

    // HEAD can move from under us — a checkout in the terminal is the usual way
    // — so it is re-read while the panel is up, whichever view is showing:
    // both of them answer a different question once HEAD has moved. Cheap: one
    // small file.
    Timer {
        interval: 3000
        repeat: true
        running: panel.visible
        onTriggered: Git.poll_branch()
    }
}
