import QtQuick
import QtQuick.Controls
import "../common"

// Application settings: a navigable two-pane window.
//
// An overlay rather than a second top-level window. It is modal over the app it
// configures, Escape dismisses it like any other sheet, and a real window would
// have to carry the frameless decoration, the corner radius and the theme all
// over again — three things this app has exactly one of.
//
// The left pane is a navbar: a field that filters it, and the categories. The
// right pane is one page per category, with a breadcrumb across the top. Each
// page is a list of what it sets; the rhythm belongs to SettingsPage and the
// primitives beside it, which is what keeps four pages looking like one window.
Item {
    id: window

    visible: Settings.open
    z: 90

    readonly property var pages: [
        { key: "general",   label: qsTr("General") },
        { key: "theme",     label: qsTr("Theme") },
        { key: "providers", label: qsTr("Providers") },
        { key: "models",    label: qsTr("Models") }
    ]
    property int current: 0

    // Everything the filter kept. Filtering the navbar is not a reason to
    // navigate away, so the page on screen is left alone even when its own row
    // is hidden.
    readonly property var shown: {
        const needle = navFilter.text.trim().toLowerCase()
        if (needle === "")
            return pages
        return pages.filter((page) => page.label.toLowerCase().indexOf(needle) >= 0)
    }

    Rectangle {
        anchors.fill: parent
        color: Theme.name === "dark" ? Qt.rgba(0, 0, 0, 0.45)
                                     : Qt.rgba(0, 0, 0, 0.28)
        MouseArea {
            anchors.fill: parent
            onClicked: Settings.hide()
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 96, 940)
        height: Math.min(parent.height - 96, 620)
        radius: 10
        color: Theme.colors.card
        border.width: 1
        border.color: Theme.colors.card_border

        // Swallows the clicks the scrim would otherwise take as "dismiss".
        MouseArea { anchors.fill: parent }

        Rectangle {
            id: bar
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 36
            topLeftRadius: card.radius
            topRightRadius: card.radius
            color: Theme.colors.header

            Rectangle {
                anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                height: 1
                color: Theme.colors.card_border
            }

            Text {
                anchors.centerIn: parent
                text: qsTr("Settings")
                color: Theme.colors.text
                font.family: Theme.mono_family
                font.pixelSize: 12
                font.weight: Font.DemiBold
            }

            IconButton {
                anchors { right: parent.right; rightMargin: 6; verticalCenter: parent.verticalCenter }
                glyph: "x"
                tooltip: qsTr("Close")
                onClicked: Settings.hide()
            }
        }

        // ---- Navbar ----------------------------------------------------------

        Rectangle {
            id: nav
            anchors { left: parent.left; top: bar.bottom; bottom: parent.bottom }
            width: 208
            bottomLeftRadius: card.radius
            color: Theme.colors.sidebar

            Rectangle {
                anchors { top: parent.top; bottom: parent.bottom; right: parent.right }
                width: 1
                color: Theme.colors.card_border
            }

            SettingsField {
                id: navFilter
                anchors { left: parent.left; right: parent.right; top: parent.top }
                anchors.margins: 10
                placeholderText: qsTr("Search settings")
            }

            Column {
                anchors {
                    left: parent.left; right: parent.right
                    top: navFilter.bottom; topMargin: 8
                    leftMargin: 8; rightMargin: 8
                }
                spacing: 2

                Repeater {
                    model: window.shown

                    Rectangle {
                        id: entry
                        required property var modelData
                        readonly property bool active:
                            window.pages[window.current].key === modelData.key

                        width: parent.width
                        height: 26
                        radius: 5
                        color: active ? Theme.colors.accent_soft
                             : entryMouse.containsMouse ? Theme.colors.hover
                             : "transparent"

                        Text {
                            anchors { left: parent.left; leftMargin: 10; verticalCenter: parent.verticalCenter }
                            text: entry.modelData.label
                            color: entry.active ? Theme.colors.accent_text
                                                : Theme.colors.text
                            font.family: Theme.sans_family
                            font.pixelSize: 12
                        }

                        MouseArea {
                            id: entryMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: window.select(entry.modelData.key)
                        }
                    }
                }
            }
        }

        // ---- Page ------------------------------------------------------------

        Item {
            anchors {
                left: nav.right; right: parent.right
                top: bar.bottom; bottom: parent.bottom
            }

            Text {
                id: breadcrumb
                anchors { left: parent.left; right: parent.right; top: parent.top }
                anchors.margins: 14
                anchors.leftMargin: 20
                textFormat: Text.RichText
                text: "Settings / <span style='color:" + Theme.colors.text + "'>"
                      + window.pages[window.current].label + "</span>"
                color: Theme.chat_colors.dim
                font.family: Theme.mono_family
                font.pixelSize: 11
            }

            Loader {
                anchors {
                    left: parent.left; right: parent.right
                    top: breadcrumb.bottom; bottom: parent.bottom
                    topMargin: 6; bottomMargin: 12
                }
                // Rebuilt on every switch rather than kept: a settings page's
                // whole state is read off disk when it is built, so a page held
                // in memory is a page that could disagree with the file.
                sourceComponent: {
                    switch (window.pages[window.current].key) {
                    case "theme": return themePage
                    case "providers": return providersPage
                    case "models": return modelsPage
                    default: return generalPage
                    }
                }
            }
        }
    }

    Component { id: generalPage; GeneralPage {} }
    Component { id: themePage; ThemePage {} }
    Component { id: providersPage; ProvidersPage {} }
    Component { id: modelsPage; ModelsPage {} }

    function select(key) {
        for (let i = 0; i < pages.length; i++) {
            if (pages[i].key === key) {
                current = i
                return
            }
        }
    }

    focus: visible
    Keys.onEscapePressed: Settings.hide()
    onVisibleChanged: if (visible) forceActiveFocus()
}
