import QtQuick

// One of the window's own buttons: a filled circle carrying its glyph only
// while the pointer is over the group.
//
// The colours are the same in both themes, as the platform's are — the traffic
// lights are the one piece of chrome that reads as the window rather than as
// the app.
Item {
    id: light

    property string kind: "close"   // close | min | max | restore
    // Set by the bar: the glyphs appear when the pointer is over *any* of the
    // three, not only over this one.
    property bool groupHovered: false
    property string tooltip

    signal clicked()

    readonly property var faces: ({
        "close":   { fill: "#ff5f57", glyphColor: "#6b0500", glyph: "x" },
        "min":     { fill: "#febc2e", glyphColor: "#7d4900", glyph: "minus" },
        "max":     { fill: "#28c840", glyphColor: "#0a5c14", glyph: "maximize-2" },
        // The green light says what the click will do, so it inverts once the
        // window is already filling the screen.
        "restore": { fill: "#28c840", glyphColor: "#0a5c14", glyph: "minimize-2" }
    })
    readonly property var face: faces[kind]

    implicitWidth: 12
    implicitHeight: 12

    Rectangle {
        anchors.fill: parent
        radius: width / 2
        color: light.face.fill
    }

    Image {
        anchors.centerIn: parent
        // The glyph sits inside the circle rather than filling it.
        width: parent.width * 0.62
        height: parent.height * 0.62
        sourceSize: Qt.size(width * 2, height * 2)
        visible: light.groupHovered
        source: Theme.icon(light.face.glyph, light.face.glyphColor)
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: light.clicked()
    }
}
