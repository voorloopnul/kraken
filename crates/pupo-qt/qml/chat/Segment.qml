import QtQuick

// One piece of a reply: prose, a code fence, or a thematic break.
//
// A reply arrives from the bridge already split into the three. A fence needs a
// card behind it and a Copy button on it, and neither can be attached to a
// `<pre>` buried inside a single rich-text item — there is no geometry to hang
// them on. A rule is split out for a different reason: the renderer draws `<hr>`
// from the widget palette and ignores every colour the markup asks for, which on
// a dark theme is a bright bar across the reply.
Item {
    id: segment

    // One entry of a block's `segments`.
    property var part

    signal copyRequested(int source)
    signal linkActivated(string url)

    implicitHeight: shape.implicitHeight

    // Width only. A Loader takes its implicit size from whatever it loaded and
    // resizes that item to its own — binding the height to the item's implicit
    // height instead re-enters that same relation and Qt reports it as a loop.
    Loader {
        id: shape
        width: segment.width
        sourceComponent: {
            switch (segment.part.kind) {
            case "code": return codePart
            case "rule": return rulePart
            default: return prosePart
            }
        }
    }

    Component {
        id: prosePart

        Text {
            textFormat: Text.RichText
            text: segment.part.html
            color: Theme.chat_colors.text
            font.family: Theme.mono_family
            font.pixelSize: Theme.chat_font_size
            wrapMode: Text.Wrap
            onLinkActivated: (url) => segment.linkActivated(url)

            // Rich text reports the link under the pointer but draws no cursor
            // for it, so the affordance needs an area of its own. It accepts no
            // buttons: the click belongs to the Text's own link handling.
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.NoButton
                hoverEnabled: true
                cursorShape: parent.hoveredLink !== "" ? Qt.PointingHandCursor
                                                       : Qt.ArrowCursor
            }
        }
    }

    Component {
        id: rulePart

        // A hairline with the same air above and below that a paragraph break
        // has, so a break between two sections reads as one.
        Item {
            implicitHeight: 13

            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: parent.width
                height: 1
                color: Theme.chat_colors.code_border
            }
        }
    }

    Component {
        id: codePart

        CodeCard {
            html: segment.part.html
            language: segment.part.language
            source: segment.part.source
            onCopyRequested: (index) => segment.copyRequested(index)
        }
    }
}
