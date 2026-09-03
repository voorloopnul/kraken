import QtQuick

// One block of the transcript, in whichever of the six shapes its kind calls
// for: a user's bubble, a reply, a reasoning row, a tool call, an info line, or
// a turn's stats footer.
//
// All six live in one file because they are one design — they share the
// transcript palette, the same collapse affordance and a common rhythm, and
// six files is how those quietly drift apart. What is separate is a reply's
// pieces (see Segment.qml), which a reply *contains* rather than is.
//
// Nothing here keeps state. The bridge rebuilds the whole list on every
// repaint, so `expanded` belongs to the transcript rather than to the row,
// which is what lets a reasoning block stay open across the repaint that is
// streaming into it.
Item {
    id: block

    // One entry of `Session.blocks`. Named `entry` rather than `data` because
    // `data` is Item's own default property.
    property var entry

    signal toggleRequested(int index, bool open)
    signal copyRequested(int source)
    signal linkActivated(string url)

    implicitHeight: shape.implicitHeight

    // Width only; the height comes back from whichever shape was loaded. See
    // Segment.qml for why binding it to the item's implicit height instead is
    // a loop.
    Loader {
        id: shape
        width: block.width
        sourceComponent: {
            switch (block.entry.kind) {
            case "user": return userShape
            case "assistant": return assistantShape
            case "thinking": return thinkingShape
            case "tool": return toolShape
            case "info": return infoShape
            default: return footerShape
            }
        }
    }

    // ---- User ---------------------------------------------------------------

    // Right-aligned in a bubble that hugs its text: the left edge follows the
    // widest line rather than the column, so a one-word message is a one-word
    // bubble. It stops short of the full width, so the side it is on stays
    // legible as the side it is on.
    Component {
        id: userShape

        Item {
            implicitHeight: bubble.height

            Rectangle {
                id: bubble
                anchors.right: parent.right
                // `implicitWidth` is the text's unwrapped width — what the
                // bubble should hug — and it does not depend on the width handed
                // back, so this settles in one pass.
                width: Math.min(parent.width - 48,
                                Math.max(message.implicitWidth,
                                         chipRow.visible ? chipRow.implicitWidth : 0) + 28)
                height: message.implicitHeight + chipRow.height + (chipRow.visible ? 22 : 16)
                radius: 10
                color: Theme.chat_colors.user_bg
                border.width: 1
                border.color: Theme.chat_colors.user_border

                Row {
                    id: chipRow
                    x: 14
                    y: 8
                    width: bubble.width - 28
                    height: visible ? implicitHeight : 0
                    spacing: 4
                    visible: chips.count > 0

                    Repeater {
                        id: chips
                        model: block.entry.attachments

                        Rectangle {
                            id: chip
                            required property var modelData
                            width: chipLabel.implicitWidth + 12
                            height: chipLabel.implicitHeight + 6
                            radius: 6
                            color: Theme.colors.header
                            border.width: 1
                            border.color: Theme.chat_colors.user_border

                            Text {
                                id: chipLabel
                                anchors.centerIn: parent
                                text: chip.modelData.name
                                color: Theme.chat_colors.dim
                                font.family: Theme.mono_family
                                font.pixelSize: Theme.caption_font_size
                            }
                        }
                    }
                }

                Text {
                    id: message
                    x: 14
                    y: chipRow.visible ? chipRow.y + chipRow.height + 6 : 8
                    width: bubble.width - 28
                    text: block.entry.text
                    color: Theme.chat_colors.text
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.chat_font_size
                    wrapMode: Text.Wrap
                }
            }
        }
    }

    // ---- Assistant ----------------------------------------------------------

    Component {
        id: assistantShape

        Column {
            spacing: 6

            Repeater {
                model: block.entry.segments

                Segment {
                    required property var modelData
                    width: parent.width
                    part: modelData
                    onCopyRequested: (source) => block.copyRequested(source)
                    onLinkActivated: (url) => block.linkActivated(url)
                }
            }
        }
    }

    // ---- Thinking -----------------------------------------------------------

    // A muted, collapsible row. Collapsed it shows the reasoning's own first
    // sentence — enough to tell whether it is worth opening, at a cost that
    // stays flat while the model streams into it.
    Component {
        id: thinkingShape

        Column {
            spacing: 4

            // The header is wrapped so the click target can fill it: a
            // MouseArea placed directly in a Row or Column would be laid out as
            // another visible child rather than sitting over one.
            Item {
                width: parent.width
                height: header.height

                Row {
                    id: header
                    width: parent.width
                    spacing: 6

                    Text {
                        id: thinkingLabel
                        text: (block.entry.expanded ? "▾" : "›") + " ✧ " + qsTr("Thinking")
                        color: Theme.chat_colors.thinking_label
                        font.family: Theme.mono_family
                        font.pixelSize: Theme.chat_font_size
                    }

                    Text {
                        width: header.width - thinkingLabel.width - header.spacing
                        visible: !block.entry.expanded
                        text: block.entry.summary
                        color: Theme.chat_colors.thinking_text
                        font.family: Theme.mono_family
                        font.pixelSize: Theme.chat_font_size
                        elide: Text.ElideRight
                    }
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: block.toggleRequested(block.entry.index,
                                                     !block.entry.expanded)
                }
            }

            Text {
                width: parent.width
                visible: block.entry.expanded
                textFormat: Text.RichText
                text: block.entry.html
                // Tinted rather than re-rendered in the muted colour: the
                // markup carries emphasis and code colours of its own, and this
                // only has to say that the whole of it is an aside.
                color: Theme.chat_colors.thinking_text
                font.family: Theme.mono_family
                font.pixelSize: Theme.chat_font_size
                wrapMode: Text.Wrap
            }
        }
    }

    // ---- Tool ---------------------------------------------------------------

    Component {
        id: toolShape

        Column {
            spacing: 6

            Text {
                width: parent.width
                text: (block.entry.expanded ? "▾ " : "▸ ") + "⚒ " + block.entry.name
                      + (block.entry.summary === "" ? "" : "  " + block.entry.summary)
                color: Theme.chat_colors.dim
                font.family: Theme.mono_family
                font.pixelSize: Theme.chat_font_size
                font.italic: true
                elide: Text.ElideRight

                // A Text lays out no children, so the target can live inside it.
                MouseArea {
                    anchors.fill: parent
                    cursorShape: block.entry.has_detail ? Qt.PointingHandCursor
                                                        : Qt.ArrowCursor
                    onClicked: {
                        if (block.entry.has_detail)
                            block.toggleRequested(block.entry.index,
                                                  !block.entry.expanded)
                    }
                }
            }

            // Arguments and output in one card *under* the header rather than
            // inside it, so the header stays the collapse target at every width.
            Rectangle {
                width: parent.width
                height: detail.implicitHeight + 20
                visible: block.entry.expanded && block.entry.detail !== ""
                radius: 8
                color: Theme.chat_colors.tool_bg
                border.width: 1
                border.color: Theme.chat_colors.tool_border

                Text {
                    id: detail
                    x: 12
                    y: 10
                    width: parent.width - 24
                    text: block.entry.detail
                    color: Theme.chat_colors.tool_detail
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    wrapMode: Text.Wrap
                }
            }
        }
    }

    // ---- Info and footer ----------------------------------------------------

    Component {
        id: infoShape

        Text {
            text: block.entry.text
            color: block.entry.error ? Theme.chat_colors.error
                                     : Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: Theme.chat_font_size
            wrapMode: Text.Wrap
        }
    }

    // The turn's stats. Smaller than the body above it, because it annotates
    // the reply rather than being more of it.
    Component {
        id: footerShape

        Text {
            text: block.entry.text
            color: Theme.chat_colors.dim
            font.family: Theme.mono_family
            font.pixelSize: Theme.caption_font_size
            wrapMode: Text.Wrap
        }
    }
}
