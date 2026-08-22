import QtQuick

// A section heading, with the hairline that closes it off.
//
// Mono where the rows around it are proportional: the structural labels are
// what give the page its scaffolded look, and the settings themselves read as
// prose.
Item {
    id: section

    property string text
    property bool first: false

    width: parent ? parent.width : 0
    implicitHeight: label.implicitHeight + (first ? 14 : 30)

    Text {
        id: label
        anchors { left: parent.left; right: parent.right; bottom: rule.top; bottomMargin: 6 }
        text: section.text
        color: Theme.colors.text
        font.family: Theme.mono_family
        font.pixelSize: 12
        font.weight: Font.DemiBold
    }

    Rectangle {
        id: rule
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 1
        color: Theme.colors.card_border
    }
}
