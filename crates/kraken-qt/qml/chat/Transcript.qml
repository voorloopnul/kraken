import QtQuick

// The conversation itself: every block of the focused session, in order.
//
// A Flickable over a Column rather than a ListView, because the bridge replaces
// the whole list on every repaint. A ListView handed a new model resets itself
// and throws the reader back to the top mid-stream; a Flickable keeps its own
// contentY while the column under it grows, which is exactly the behaviour a
// transcript wants. Nothing is virtualised, and that matches what it replaces —
// the widget port rendered the whole conversation as one text document.
Flickable {
    id: transcript

    // `Session.blocks`.
    property var blocks: []
    // The reading column's cap. Prose past about this width is measurably
    // harder to follow, and the panel can be much wider than that.
    property int columnWidth: 1000

    signal toggleRequested(int index, bool open)
    signal copyRequested(int source)
    signal linkActivated(string url)

    contentWidth: width
    contentHeight: column.height + 24
    boundsBehavior: Flickable.StopAtBounds
    clip: true

    // How far off the bottom still counts as being at the bottom, so a stray
    // pixel of rounding does not read as the reader having scrolled up.
    readonly property int slack: 4
    readonly property bool atBottom: contentHeight <= height
                                     || contentY >= contentHeight - height - slack
    // Follow the stream until the reader scrolls away from the bottom, and pick
    // it up again the moment they come back.
    property bool stick: true

    onContentYChanged: if (moving || dragging) stick = atBottom
    onMovementEnded: stick = atBottom
    onContentHeightChanged: if (stick) scrollToEnd()
    onHeightChanged: if (stick) scrollToEnd()

    function scrollToEnd() {
        contentY = Math.max(0, contentHeight - height)
    }

    // The gap above a block.
    //
    // Reasoning and tool rows are one running commentary, so consecutive ones
    // sit close together; a footer annotates the reply it follows and hugs it.
    // Everything else is a new turn in the conversation and gets the full break.
    function gapBefore(index) {
        if (index <= 0)
            return 0
        const kind = blocks[index].kind
        const previous = blocks[index - 1].kind
        const activity = (k) => k === "thinking" || k === "tool"
        if (kind === "footer" || (activity(kind) && activity(previous)))
            return 8
        return 16
    }

    Column {
        id: column
        x: Math.max(0, (transcript.width - width) / 2)
        y: 12
        width: Math.min(transcript.width - 24, transcript.columnWidth)
        spacing: 0

        Repeater {
            model: transcript.blocks

            Item {
                id: slot
                required property var modelData
                required property int index
                readonly property int gap: transcript.gapBefore(index)

                width: column.width
                height: body.height + gap

                Block {
                    id: body
                    y: slot.gap
                    width: slot.width
                    entry: slot.modelData
                    onToggleRequested: (at, open) => transcript.toggleRequested(at, open)
                    onCopyRequested: (source) => transcript.copyRequested(source)
                    onLinkActivated: (url) => transcript.linkActivated(url)
                }
            }
        }
    }
}
