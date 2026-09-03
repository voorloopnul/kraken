import QtQuick

// One fenced code block: the highlighted source on a rounded card, with a Copy
// button over its top-right corner.
//
// The card is an item of its own rather than markup inside the reply, because a
// button has to have somewhere to sit and a `<pre>` buried in a rich-text Text
// has no geometry anything outside it can ask for — which is why the bridge
// hands the reply over already split into prose and fences.
//
// Long lines scroll sideways instead of wrapping: wrapped code loses the
// indentation that is half of how it is read.
Item {
    id: card

    // The `<pre>` markup the bridge rendered, and the running index that
    // `Session.copy_code` answers for.
    property string html
    property string language
    property int source: -1

    signal copyRequested(int source)

    implicitHeight: frame.height

    Rectangle {
        id: frame
        width: parent.width
        height: scroller.height + 20
        radius: 8
        color: Theme.chat_colors.code_bg
        border.width: 1
        border.color: Theme.chat_colors.code_border

        Flickable {
            id: scroller
            x: 14
            y: 10
            width: parent.width - 28
            height: body.implicitHeight
            contentWidth: Math.max(body.implicitWidth, width)
            contentHeight: height
            flickableDirection: Flickable.HorizontalFlick
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            Text {
                id: body
                textFormat: Text.RichText
                text: card.html
                color: Theme.chat_colors.text
                font.family: Theme.mono_family
                font.pixelSize: Theme.chat_font_size
                // Off, so the Flickable above measures the code's true width
                // and scrolls to it.
                wrapMode: Text.NoWrap
            }
        }

        // Flat against the card rather than outlined: it overlaps the first
        // code line, and a bordered chip there would read as part of the code.
        Rectangle {
            id: copyButton
            anchors { top: parent.top; right: parent.right; topMargin: 6; rightMargin: 6 }
            width: copyLabel.implicitWidth + 16
            height: copyLabel.implicitHeight + 4
            radius: 4
            visible: card.source >= 0
            color: copyMouse.containsMouse ? Theme.colors.hover
                                           : Theme.chat_colors.code_bg

            Text {
                id: copyLabel
                anchors.centerIn: parent
                text: copyMouse.copied ? qsTr("Copied") : qsTr("Copy")
                color: copyMouse.containsMouse ? Theme.colors.text
                                               : Theme.chat_colors.dim
                font.family: Theme.mono_family
                font.pixelSize: Theme.caption_font_size
            }

            MouseArea {
                id: copyMouse
                property bool copied: false
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: {
                    card.copyRequested(card.source)
                    copied = true
                    revert.restart()
                }
                Timer { id: revert; interval: 1200; onTriggered: copyMouse.copied = false }
            }
        }
    }
}
