import QtQuick

// A rounded container to group content.
//
// `flat` makes it a surface instead: the same background, but no border and no
// radius, for a card that meets its neighbours edge to edge and lets the
// divider between them draw the only line. A header added through `header` sits
// outside the padding rather than in it — a panel's top strip is a surface of
// its own and runs the full width, while everything below it stays inset.
Rectangle {
    id: card

    property bool flat: false
    property int padding: 12
    default property alias content: contentArea.data
    property alias header: headerArea.data

    color: Theme.colors.card
    radius: flat ? 0 : 8
    border.width: flat ? 0 : 1
    border.color: flat ? "transparent" : Theme.colors.card_border

    Column {
        anchors.fill: parent
        spacing: 0

        Item {
            id: headerArea
            width: parent.width
            height: childrenRect.height
        }

        Item {
            id: contentArea
            width: parent.width
            height: parent.height - headerArea.height
        }
    }
}
