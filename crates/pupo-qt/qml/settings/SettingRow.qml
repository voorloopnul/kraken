import QtQuick
import "../common"

// One setting: what it is and what it does on the left, the control on the
// right.
//
// Every row on every page is one of these, which is what keeps the vertical
// rhythm and the label/control split identical as the pages fill in — a page
// that laid its own rows out would drift from its neighbours within a week.
Item {
    id: row

    property string title
    property string description
    // The control's column. Fixed rather than shared with the text, so the
    // controls line up down the page whatever their labels say.
    property int controlWidth: 260
    default property alias control: controlArea.data

    // A Column gives its children no width, so a row that did not take its
    // parent's would lay its text out in nothing and come out a hairline tall.
    width: parent ? parent.width : 0
    implicitHeight: Math.max(text.implicitHeight, controlArea.childrenRect.height) + 18

    Column {
        id: text
        anchors {
            left: parent.left
            right: controlArea.left
            rightMargin: 24
            top: parent.top
            topMargin: 6
        }
        spacing: 3

        Text {
            width: parent.width
            text: row.title
            color: Theme.colors.text
            font.family: Theme.sans_family
            font.pixelSize: 13
            wrapMode: Text.Wrap
        }

        Text {
            width: parent.width
            visible: row.description !== ""
            text: row.description
            color: Theme.chat_colors.dim
            font.family: Theme.sans_family
            font.pixelSize: 11
            wrapMode: Text.Wrap
            lineHeight: 1.25
        }
    }

    Item {
        id: controlArea
        anchors { right: parent.right; top: parent.top; topMargin: 4 }
        width: row.controlWidth
        height: childrenRect.height
    }
}
