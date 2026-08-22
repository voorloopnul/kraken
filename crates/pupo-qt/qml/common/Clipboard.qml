import QtQuick

// The system clipboard, in both directions.
//
// QML has no clipboard API and the Rust bindings cannot reach
// QGuiApplication::clipboard, so both directions go through the one element that
// already owns one: a TextEdit holding nothing but the string in flight, emptied
// again immediately so a stale reply is never left sitting in the tree.
Item {
    id: clip

    visible: false
    width: 0
    height: 0

    function copy(text) {
        if (!text)
            return
        holder.text = text
        holder.selectAll()
        holder.copy()
        holder.text = ""
    }

    function paste() {
        holder.text = ""
        holder.paste()
        const text = holder.text
        holder.text = ""
        return text
    }

    TextEdit {
        id: holder
        visible: false
        width: 0
        height: 0
    }
}
