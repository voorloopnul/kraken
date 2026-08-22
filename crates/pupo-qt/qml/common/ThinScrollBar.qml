import QtQuick
import QtQuick.Controls

// The app's one scrollbar: a rounded handle on a transparent track, no steppers.
//
// Controls' default carries the platform style's arrows and groove, which on a
// themed frameless window reads as a piece of someone else's chrome. Every
// scrolling surface in the app attaches one of these instead, so a list, a
// transcript and a diff all scroll the same way.
//
// It hides itself when there is nothing to scroll rather than sitting there
// full-length, which is the only honest thing a full-length handle could mean.
ScrollBar {
    id: control

    policy: size < 1 ? ScrollBar.AsNeeded : ScrollBar.AlwaysOff
    // Wide enough to grab, narrow enough to overlay content rather than take a
    // column out of the layout.
    implicitWidth: 10
    implicitHeight: 10
    padding: 0

    background: null

    // The handle's implicit size along its own axis is its floor: a handle
    // shorter than this is a target nobody can hit, however long the content
    // behind it happens to be.
    contentItem: Rectangle {
        readonly property bool vertical: control.orientation === Qt.Vertical
        implicitWidth: vertical ? 10 : 24
        implicitHeight: vertical ? 24 : 10
        radius: 5
        color: control.pressed || control.hovered
               ? (Theme.name === "dark" ? "#4a4e58" : "#b6b3ac")
               : (Theme.name === "dark" ? "#3a3d45" : "#ccc9c3")
    }
}
