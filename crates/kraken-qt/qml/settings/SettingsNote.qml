import QtQuick

// A paragraph of explanation between the rows — what a page is writing to, or
// what it has just done.
Text {
    width: parent ? parent.width : 0
    topPadding: 8
    color: Theme.chat_colors.dim
    font.family: Theme.sans_family
    font.pixelSize: 11
    wrapMode: Text.Wrap
    lineHeight: 1.3
}
