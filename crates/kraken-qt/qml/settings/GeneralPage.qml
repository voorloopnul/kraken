import QtQuick

// Nothing to set here yet. The page exists because the navbar reads better with
// the category it will hold than with a gap where it will go.
SettingsPage {
    SettingsSection { text: qsTr("General"); first: true }
    SettingsNote {
        text: qsTr("No general settings yet. What Kraken remembers between runs — "
                   + "the theme, the two font sizes, which workspace was open and "
                   + "which panels were showing — it remembers on its own.")
    }
}
