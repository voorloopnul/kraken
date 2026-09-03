import QtQuick
import "../common"

// A number with a − and a + beside it.
//
// A stepper rather than a free field: every number in this window is a size
// with a floor and a ceiling, and typing one is an invitation to type one that
// is refused.
Row {
    id: stepper

    property int value: 0
    property int from: 0
    property int to: 100
    property string suffix: ""

    signal changed(int value)

    spacing: 4

    function step(delta) {
        const next = Math.max(from, Math.min(to, value + delta))
        if (next !== value)
            stepper.changed(next)
    }

    TextButton {
        text: "−"
        fontSize: 13
        enabled: stepper.value > stepper.from
        onClicked: stepper.step(-1)
    }

    Text {
        anchors.verticalCenter: parent.verticalCenter
        width: 52
        horizontalAlignment: Text.AlignHCenter
        text: stepper.value + stepper.suffix
        color: Theme.colors.text
        font.family: Theme.mono_family
        font.pixelSize: 12
    }

    TextButton {
        text: "+"
        fontSize: 13
        enabled: stepper.value < stepper.to
        onClicked: stepper.step(1)
    }
}
