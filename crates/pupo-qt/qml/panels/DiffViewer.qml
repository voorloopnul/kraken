import QtQuick
import QtQuick.Controls
import "../common"

// One file's diff, over a dimmed app.
//
// A sheet rather than a window: it belongs to the workspace it came from, it is
// dismissed with Escape like any other overlay, and a second top-level window
// would have to carry the frame, the theme and the corner radius all over again.
//
// The rows arrive already lexed and coloured — the gutter numbers, the +/− mark
// and the syntax spans inside each line. Nothing here decides a colour; it lays
// out what it is handed and scrolls it.
Item {
    id: sheet

    visible: Diff.viewer_open
    // Above everything, including the panel that raised it.
    z: 100

    readonly property real cellWidth: probe.implicitWidth / 10
    readonly property real rowHeight: probe.implicitHeight

    // Measured off a rendered line for the same reason the terminal does it:
    // `FontMetrics.advanceWidth` is a method, and a binding on a method never
    // hears that the bundled font finally loaded.
    Text {
        id: probe
        visible: false
        text: "MMMMMMMMMM"
        font.family: Theme.mono_family
        font.pixelSize: 11
    }

    // The scrim carries its own alpha: it is what makes the sheet modal, so it
    // is a transparency over the app rather than a tint of it. It also eats
    // every click that misses the card, which is what closes the sheet.
    Rectangle {
        anchors.fill: parent
        color: Diff.viewer_scrim

        MouseArea {
            anchors.fill: parent
            onClicked: Diff.close_viewer()
        }
    }

    Rectangle {
        id: card
        anchors.centerIn: parent
        width: Math.min(parent.width - 80, 1100)
        height: parent.height - 80
        radius: 10
        color: Theme.colors.card
        border.width: 1
        border.color: Theme.colors.card_border

        // Swallows the clicks the scrim would otherwise take as "dismiss".
        MouseArea { anchors.fill: parent }

        Rectangle {
            id: head
            anchors { left: parent.left; right: parent.right; top: parent.top }
            height: 40
            radius: card.radius
            color: Theme.colors.header

            // The header's own bottom corners are square — the radius above is
            // there for the card's top two, and a rounded strip in the middle of
            // a card reads as a chip sitting on it.
            Rectangle {
                anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                height: parent.radius
                color: parent.color
            }

            Rectangle {
                anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
                height: 1
                color: Theme.colors.card_border
            }

            Text {
                id: letter
                anchors { left: parent.left; leftMargin: 14; verticalCenter: parent.verticalCenter }
                text: Diff.viewer_letter
                color: Diff.viewer_letter_color
                font.family: Theme.mono_family
                font.pixelSize: 12
                font.weight: Font.DemiBold
            }

            Text {
                anchors {
                    left: letter.right; leftMargin: 8
                    right: subtitle.left; rightMargin: 10
                    verticalCenter: parent.verticalCenter
                }
                text: Diff.viewer_path
                color: Theme.colors.text
                font.family: Theme.mono_family
                font.pixelSize: 12
                // The filename is the informative end of a path, so the leading
                // directories are what gets eaten.
                elide: Text.ElideLeft
            }

            Text {
                id: subtitle
                anchors { right: dismiss.left; rightMargin: 10; verticalCenter: parent.verticalCenter }
                text: Diff.viewer_subtitle
                color: Diff.viewer_dim_color
                font.family: Theme.mono_family
                font.pixelSize: 11
            }

            IconButton {
                id: dismiss
                anchors { right: parent.right; rightMargin: 8; verticalCenter: parent.verticalCenter }
                glyph: "x"
                tooltip: qsTr("Close")
                onClicked: Diff.close_viewer()
            }
        }

        // Nothing to lay out as lines: a binary file, an unreadable one, or a
        // diff that turned out to hold none.
        Text {
            anchors.centerIn: parent
            visible: Diff.viewer_message !== ""
            text: Diff.viewer_message
            color: Diff.viewer_dim_color
            font.family: Theme.mono_family
            font.pixelSize: 12
        }

        Flickable {
            id: body
            anchors {
                left: parent.left; right: parent.right
                top: head.bottom; bottom: parent.bottom
                margins: 1
            }
            visible: Diff.viewer_message === ""
            clip: true
            // Diff lines never wrap — a wrapped line loses the indentation that
            // is half of how a diff is read — so a long one scrolls sideways.
            contentWidth: Math.max(width, gutter.width + 16
                                          + Diff.viewer_columns * sheet.cellWidth)
            contentHeight: Diff.viewer_rows.length * sheet.rowHeight
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}
            ScrollBar.horizontal: ThinScrollBar {}

            Repeater {
                model: Diff.viewer_rows

                Item {
                    id: line
                    required property var modelData
                    required property int index

                    y: index * sheet.rowHeight
                    width: body.contentWidth
                    height: sheet.rowHeight

                    Rectangle {
                        anchors.fill: parent
                        visible: line.modelData.background !== ""
                        color: line.modelData.background === ""
                               ? "transparent" : line.modelData.background
                    }

                    // Old and new line numbers, right-aligned in their own
                    // columns so the digits line up down the page.
                    Row {
                        id: numbers
                        x: 8
                        height: parent.height
                        spacing: 8

                        Text {
                            width: Diff.viewer_digits * sheet.cellWidth
                            height: parent.height
                            horizontalAlignment: Text.AlignRight
                            text: line.modelData.old_no
                            color: Diff.viewer_gutter_color
                            font: probe.font
                        }
                        Text {
                            width: Diff.viewer_digits * sheet.cellWidth
                            height: parent.height
                            horizontalAlignment: Text.AlignRight
                            text: line.modelData.new_no
                            color: Diff.viewer_gutter_color
                            font: probe.font
                        }
                    }

                    Text {
                        id: mark
                        x: numbers.x + numbers.width + 8
                        height: parent.height
                        width: sheet.cellWidth
                        text: line.modelData.mark
                        color: line.modelData.mark_color
                        font: probe.font
                    }

                    // The line itself, as the coloured runs the lexer produced.
                    Row {
                        x: mark.x + mark.width
                        height: parent.height

                        Repeater {
                            model: line.modelData.runs

                            Text {
                                required property var modelData
                                height: parent.height
                                text: modelData.text
                                color: modelData.color
                                font.family: probe.font.family
                                font.pixelSize: probe.font.pixelSize
                                font.italic: modelData.italic
                                textFormat: Text.PlainText
                            }
                        }
                    }
                }
            }
        }

        // The gutter's width, measured once so the body and every row agree on
        // where the code starts.
        Item {
            id: gutter
            visible: false
            width: 8 + Diff.viewer_digits * sheet.cellWidth * 2 + 8 + 8 + sheet.cellWidth
        }
    }

    // Escape closes it, which is why the sheet takes focus while it is up.
    focus: visible
    Keys.onEscapePressed: Diff.close_viewer()
    onVisibleChanged: if (visible) forceActiveFocus()
}
