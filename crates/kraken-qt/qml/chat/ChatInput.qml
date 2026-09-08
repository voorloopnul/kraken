import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import "../common"

// The composer: attachment chips, the prompt field, and a footer of controls.
//
// It grows with what is typed and stops at a cap, because the prompt shares the
// panel with the transcript and a box that grew without limit would push the
// conversation off the top of the screen.
//
// Nothing here holds the conversation's state: the model and effort pills read
// the bridge, and the menus behind them open only once the agent has answered
// what the choices are — a menu that opened first and filled in afterwards
// would be a menu you had to open twice.
Rectangle {
    id: composer

    property bool busy: false
    property bool stopArmed: false

    signal submitted(string text, var files)

    // Kept for the panel around it: the composer's height is content-driven and
    // the layout above it needs to know what is left.
    implicitHeight: layout.implicitHeight + 10

    radius: 10
    color: Theme.colors.header
    border.width: 1
    border.color: Theme.colors.card_border

    function insertText(text) {
        prompt.insert(prompt.cursorPosition, text)
    }

    function submit() {
        const text = prompt.text
        if (text.trim() === "" && Session.attachments.length === 0)
            return
        prompt.clear()
        composer.submitted(text, [])
    }

    // Files dropped anywhere on the box, not only on the chip row: the target
    // people aim at is the composer, and a strip they have to hit instead is a
    // strip they miss.
    DropArea {
        anchors.fill: parent
        onDropped: function (drop) {
            for (const url of drop.urls)
                Session.attach_file(url)
            drop.accept()
        }
    }

    Column {
        id: layout
        anchors { fill: parent; margins: 6; bottomMargin: 4 }
        spacing: 4

        // Chips for what the next prompt will carry. Hidden entirely while
        // nothing is attached, so the box does not reserve a strip of nothing.
        Flow {
            id: chipRow
            width: parent.width
            spacing: 4
            visible: chips.count > 0
            height: visible ? implicitHeight : 0

            Repeater {
                id: chips
                model: Session.attachments

                Rectangle {
                    id: chip
                    required property var modelData
                    required property int index

                    width: chipLabel.implicitWidth + thumb.width + remove.width + 16
                    height: 22
                    radius: 6
                    color: Theme.colors.card
                    border.width: 1
                    border.color: Theme.colors.card_border

                    Image {
                        id: thumb
                        anchors { left: parent.left; leftMargin: 4; verticalCenter: parent.verticalCenter }
                        width: chip.modelData.preview === "" ? 0 : 16
                        height: 16
                        visible: width > 0
                        fillMode: Image.PreserveAspectCrop
                        // Sized down on load: a chip does not need a photograph's
                        // worth of pixels, and decoding one at full size for a
                        // 16px square is the whole cost of attaching it.
                        sourceSize: Qt.size(32, 32)
                        source: chip.modelData.preview
                        asynchronous: true
                        clip: true
                    }

                    Text {
                        id: chipLabel
                        anchors {
                            left: thumb.right; leftMargin: thumb.visible ? 5 : 2
                            verticalCenter: parent.verticalCenter
                        }
                        text: chip.modelData.name
                        color: Theme.colors.text
                        font.family: Theme.mono_family
                        font.pixelSize: Theme.caption_font_size
                        elide: Text.ElideMiddle
                        // Long enough to recognise a file by, short enough that
                        // three chips still fit across the box.
                        width: Math.min(implicitWidth, 160)
                    }

                    IconButton {
                        id: remove
                        anchors { left: chipLabel.right; leftMargin: 2; verticalCenter: parent.verticalCenter }
                        implicitWidth: 16
                        implicitHeight: 16
                        glyphSize: 10
                        radius: 4
                        glyph: "x"
                        onClicked: Session.remove_attachment(chip.index)
                    }
                }
            }
        }

        Flickable {
            id: promptScroll
            width: parent.width
            // Three-ish lines empty, about sixteen before it stops growing.
            height: Math.max(64, Math.min(320, prompt.implicitHeight))
            contentWidth: width
            contentHeight: prompt.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            clip: true

            ScrollBar.vertical: ThinScrollBar {}

            TextArea.flickable: TextArea {
                id: prompt
                placeholderText: qsTr("Follow-up on this task, @ for mentions, / for commands")
                color: Theme.chat_colors.text
                placeholderTextColor: Theme.chat_colors.dim
                font.family: Theme.mono_family
                font.pixelSize: Theme.chat_font_size
                wrapMode: TextArea.Wrap
                selectByMouse: true
                background: null
                padding: 4

                // Paste is a picture first and text second: a clipboard
                // holding an image has no text to paste anyway, so asking the
                // bridge costs the text case nothing and the keystroke falls
                // through untouched when the answer is no.
                Keys.onPressed: function (event) {
                    if (event.matches(StandardKey.Paste) && Session.paste_image())
                        event.accepted = true
                }

                // Enter sends and Shift+Enter breaks the line: the box is a
                // prompt first and a text editor second, and the common case
                // should not need a second key.
                Keys.onReturnPressed: function (event) {
                    if (event.modifiers & (Qt.ShiftModifier | Qt.KeypadModifier)) {
                        event.accepted = false
                        return
                    }
                    composer.submit()
                }
                Keys.onEnterPressed: function (event) {
                    if (event.modifiers & Qt.ShiftModifier) {
                        event.accepted = false
                        return
                    }
                    composer.submit()
                }
            }
        }

        Item {
            id: footer
            width: parent.width
            height: sendButton.implicitHeight + 2

            Row {
                anchors { left: parent.left; leftMargin: 4; verticalCenter: parent.verticalCenter }
                spacing: 6

                TextButton {
                    text: "+"
                    tooltip: qsTr("Attach images or files")
                    onClicked: fileDialog.open()
                }

                Text {
                    text: "|"
                    color: Theme.colors.card_border
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    anchors.verticalCenter: parent.verticalCenter
                }

                TextButton {
                    id: modelPill
                    text: (Session.model_label === "" ? qsTr("Model")
                                                      : Session.model_label) + "  ⌄"
                    tooltip: qsTr("Switch model")
                    onClicked: {
                        modelMenu.pending = true
                        Session.request_models()
                    }
                }
            }

            Row {
                anchors { right: parent.right; rightMargin: 4; verticalCenter: parent.verticalCenter }
                spacing: 6

                TextButton {
                    id: effortPill
                    visible: Session.effort_supported
                    text: (Session.effort_label === "" ? qsTr("Effort")
                                                       : Session.effort_label) + "  ⌄"
                    tooltip: qsTr("Reasoning effort")
                    onClicked: {
                        effortMenu.pending = true
                        Session.request_effort()
                    }
                }

                Text {
                    visible: effortPill.visible
                    text: "|"
                    color: Theme.colors.card_border
                    font.family: Theme.mono_family
                    font.pixelSize: Theme.secondary_font_size
                    anchors.verticalCenter: parent.verticalCenter
                }

                TextButton {
                    id: sendButton
                    text: qsTr("Send")
                    enabled: prompt.text.trim() !== "" || Session.attachments.length > 0
                    onClicked: composer.submit()
                }
            }
        }
    }

    // ---- Menus ---------------------------------------------------------------

    // Both open on the bridge's ready signal rather than on the click, because
    // the list behind them is a round trip to the agent. `pending` is what keeps
    // a list that arrives after the reader has moved on from opening a menu
    // over whatever they are looking at now.
    PickerPopup {
        id: modelMenu
        property bool pending: false
        parent: composer
        x: 4
        y: -height - 4
        searchable: true
        emptyText: qsTr("No models configured — see Settings › Models")
        entries: Session.models.map(function (model) {
            return {
                label: model.name === "" ? model.id : model.name,
                detail: model.provider,
                current: model.current,
                payload: model
            }
        })
        onPicked: function (model) {
            Session.set_model(model.provider, model.id)
            close()
        }
    }

    PickerPopup {
        id: effortMenu
        property bool pending: false
        parent: composer
        x: composer.width - width - 4
        y: -height - 4
        emptyText: qsTr("This model has no reasoning levels")
        entries: Session.effort_levels.map(function (level) {
            return {
                label: level,
                detail: "",
                current: level === Session.effort_label,
                payload: level
            }
        })
        onPicked: function (level) {
            Session.set_effort(level)
            close()
        }
    }

    Connections {
        target: Session
        function onModels_ready() {
            if (!modelMenu.pending)
                return
            modelMenu.pending = false
            modelMenu.open()
        }
        function onEffort_ready() {
            if (!effortMenu.pending)
                return
            effortMenu.pending = false
            effortMenu.open()
        }
    }

    FileDialog {
        id: fileDialog
        title: qsTr("Attach files")
        fileMode: FileDialog.OpenFiles
        onAccepted: {
            for (const url of selectedFiles)
                Session.attach_file(url.toString())
        }
    }
}
