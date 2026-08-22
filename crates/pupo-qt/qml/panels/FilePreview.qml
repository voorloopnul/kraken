import QtQuick
import QtQuick.Controls
import "../common"

// One file's contents, over a dimmed app.
//
// The diff sheet's twin, and built to look like it: same card, same scrim, same
// gutter, same lexer runs. A file read here and the same file read as a diff are
// the same reading surface, which is the point — two sheets that coloured the
// same code differently would be two sheets nobody trusts.
//
// It caps at the same number of lines the diff sheet caps at, and lays them out
// the same way. A file longer than that says so along the bottom rather than in
// place of the body — the lines that did arrive are still worth reading.
Item {
    id: sheet

    visible: Files.preview_open
    // Above everything, including the panel that raised it.
    z: 100

    readonly property real cellWidth: probe.implicitWidth / 10
    readonly property real rowHeight: probe.implicitHeight

    // Measured off a rendered line for the same reason the diff sheet and the
    // terminal do it: `FontMetrics.advanceWidth` is a method, and a binding on
    // a method never hears that the bundled font finally loaded.
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
        color: Files.preview_scrim

        MouseArea {
            anchors.fill: parent
            onClicked: Files.close_preview()
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
            // there for the card's top two, and a rounded strip in the middle
            // of a card reads as a chip sitting on it.
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

            Image {
                id: glyph
                anchors { left: parent.left; leftMargin: 14; verticalCenter: parent.verticalCenter }
                width: 14
                height: 14
                sourceSize: Qt.size(28, 28)
                smooth: true
                source: Theme.icon("file", Files.dim_color)
            }

            Text {
                anchors {
                    left: glyph.right; leftMargin: 8
                    right: subtitle.left; rightMargin: 10
                    verticalCenter: parent.verticalCenter
                }
                text: Files.preview_path
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
                text: Files.preview_subtitle
                color: Files.dim_color
                font.family: Theme.mono_family
                font.pixelSize: 11
            }

            IconButton {
                id: dismiss
                anchors { right: parent.right; rightMargin: 8; verticalCenter: parent.verticalCenter }
                glyph: "x"
                tooltip: qsTr("Close")
                onClicked: Files.close_preview()
            }
        }

        // Still reading. On a remote workspace the bytes are a round trip
        // away, and a sheet that stayed blank until they landed would read as
        // a click that did nothing.
        Text {
            anchors.centerIn: parent
            visible: Files.preview_kind === "loading"
            text: qsTr("Reading…")
            color: Files.dim_color
            font.family: Theme.mono_family
            font.pixelSize: 12
        }

        // Nothing to lay out as lines: a binary file, an empty one, one past the
        // size budget, or one that would not open.
        Text {
            anchors.centerIn: parent
            width: parent.width - 80
            horizontalAlignment: Text.AlignHCenter
            visible: Files.preview_kind === "message"
            text: Files.preview_message
            color: Files.dim_color
            font.family: Theme.mono_family
            font.pixelSize: 12
            wrapMode: Text.Wrap
        }

        // A picture, shown at its own size until it does not fit and then
        // fitted. Blowing a 16x16 favicon up to fill the card would be a
        // preview that lies about what the file is.
        Image {
            anchors {
                left: parent.left; right: parent.right
                top: head.bottom; bottom: parent.bottom
                margins: 20
            }
            visible: Files.preview_kind === "image"
            source: Files.preview_kind === "image" ? Files.preview_url : ""
            fillMode: Image.PreserveAspectFit
            // Never up, only down: `PreserveAspectFit` alone would stretch a
            // small image across the whole card.
            mipmap: true
            asynchronous: true
            horizontalAlignment: Image.AlignHCenter
            verticalAlignment: Image.AlignVCenter
        }

        // The text body. Laid out exactly as the diff sheet lays its rows
        // out, down to the gutter arithmetic, because it is the same reading
        // surface — and capped at the same few thousand lines, which is what
        // keeps laying every row out an honest thing to do.
        Flickable {
            id: body
            anchors {
                left: parent.left; right: parent.right
                top: head.bottom; bottom: parent.bottom
                margins: 1
            }
            visible: Files.preview_kind === "text"
            clip: true
            // Lines never wrap — a wrapped line loses the indentation that is
            // half of how code is read — so a long one scrolls sideways.
            contentWidth: Math.max(width, gutter.width + 16
                                          + Files.preview_columns * sheet.cellWidth)
            contentHeight: Files.preview_rows.length * sheet.rowHeight
            boundsBehavior: Flickable.StopAtBounds

            ScrollBar.vertical: ThinScrollBar {}
            ScrollBar.horizontal: ThinScrollBar {}

            Repeater {
                model: Files.preview_rows

                Item {
                    id: line
                    required property var modelData
                    required property int index

                    y: index * sheet.rowHeight
                    width: body.contentWidth
                    height: sheet.rowHeight

                    // The line number, right-aligned in its own column so the
                    // digits line up down the page.
                    Text {
                        id: number
                        x: 8
                        width: Files.preview_digits * sheet.cellWidth
                        height: parent.height
                        horizontalAlignment: Text.AlignRight
                        text: line.modelData.no
                        color: Files.preview_gutter_color
                        font: probe.font
                    }

                    // The line itself, as the coloured runs the lexer produced.
                    Row {
                        x: number.x + number.width + 16
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

        // A file that was cut short, said along the bottom rather than in place
        // of the body: the lines that did arrive are still worth reading.
        Rectangle {
            anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
            anchors.margins: 1
            height: 22
            visible: Files.preview_kind === "text" && Files.preview_message !== ""
            color: Theme.colors.header

            Text {
                anchors.centerIn: parent
                text: Files.preview_message
                color: Files.dim_color
                font.family: Theme.mono_family
                font.pixelSize: 10
            }
        }

        // The gutter's width, measured once so the body and every row agree on
        // where the text starts.
        Item {
            id: gutter
            visible: false
            width: 8 + Files.preview_digits * sheet.cellWidth + 16
        }
    }

    // Escape closes it, which is why the sheet takes focus while it is up.
    focus: visible
    Keys.onEscapePressed: Files.close_preview()
    onVisibleChanged: if (visible) forceActiveFocus()
}
