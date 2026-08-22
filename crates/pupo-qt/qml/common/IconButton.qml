import QtQuick

// A square button carrying one Lucide glyph.
//
// Every piece of chrome in the app is one of these, which is why the glyph is
// named rather than drawn: the icons are recoloured from a single vendored set
// (see Theme.icon), so a theme change repaints all of them at once instead of a
// dozen little paint routines drifting apart in weight and corner radius.
Item {
    id: control

    property string glyph
    property string tooltip
    property bool checkable: false
    property bool checked: false
    // Painted behind the glyph while the pointer is over the button.
    property color hoverColor: Theme.colors.hover
    // What a checked button is filled with, and what its glyph turns to on
    // that fill: a glyph in the idle grey does not survive an accent ground.
    property color checkedColor: Theme.colors.accent
    property color checkedGlyphColor: Theme.colors.accent_on
    property color glyphColor: Theme.name === "dark" ? "#9a9da5" : "#5a5d65"
    property color hoverGlyphColor: Theme.colors.text
    property int radius: 6
    property int glyphSize: 18

    signal clicked()
    signal rightClicked()

    implicitWidth: 28
    implicitHeight: 28

    Rectangle {
        anchors.fill: parent
        radius: control.radius
        color: control.checked ? control.checkedColor
             : mouse.containsMouse ? control.hoverColor
             : "transparent"
    }

    Image {
        anchors.centerIn: parent
        width: control.glyphSize
        height: control.glyphSize
        // Rendered at twice the logical size so the strokes stay clean where
        // the desktop is scaled.
        sourceSize: Qt.size(control.glyphSize * 2, control.glyphSize * 2)
        smooth: true
        visible: control.glyph !== ""
        source: control.glyph === "" ? "" : Theme.icon(
            control.glyph,
            control.checked ? control.checkedGlyphColor
                            : mouse.containsMouse ? control.hoverGlyphColor
                                                  : control.glyphColor)
    }

    MouseArea {
        id: mouse
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: function (event) {
            if (event.button === Qt.RightButton) {
                control.rightClicked()
                return
            }
            if (control.checkable)
                control.checked = !control.checked
            control.clicked()
        }
    }

    ToolTipLabel {
        text: control.tooltip
        visible: mouse.containsMouse && control.tooltip !== ""
    }
}
