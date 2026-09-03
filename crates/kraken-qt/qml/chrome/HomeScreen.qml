import QtQuick

// The logo shown when no workspace is open, on a background of its own.
//
// The home screen keeps the cream the rest of the app used to be painted in: it
// is one logo on an empty window rather than a working surface, and it is the
// one place the warmth was worth keeping. It rounds the window's bottom-right
// corner because on this screen the side strip that would otherwise sit there
// is hidden, so the corner is the home screen's own.
Rectangle {
    id: home
    property int cornerRadius: 0

    color: Theme.colors.home
    bottomRightRadius: home.cornerRadius

    Image {
        anchors.centerIn: parent
        source: "qrc:/assets/images/" + Theme.name + ".png"
        fillMode: Image.PreserveAspectFit
        width: Math.max(240, Math.min(home.width - 96, 420))
        height: Math.max(180, Math.min(home.height - 96, 420))
        smooth: true
        mipmap: true

        // A missing asset should read as the app's name rather than as a
        // broken-image glyph.
        Text {
            anchors.centerIn: parent
            visible: parent.status === Image.Error || parent.status === Image.Null
            text: "Kraken"
            color: Theme.colors.text
            font.family: Theme.mono_family
            font.pixelSize: 32
        }
    }
}
