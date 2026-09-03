import QtQuick

// A heading inside a section — one provider's rows, say.
Text {
    property bool first: false

    width: parent ? parent.width : 0
    topPadding: first ? 12 : 18
    bottomPadding: 2
    color: Theme.chat_colors.dim
    font.family: Theme.mono_family
    font.pixelSize: 11
    font.weight: Font.DemiBold
}
