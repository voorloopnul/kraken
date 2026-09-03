import QtQuick
import "../common"

// Credentials: OpenRouter, a local model server, and the ChatGPT (Codex) plan.
//
// Every row here edits a file that belongs to **pi** — `auth.json` for
// credentials, `models.json` for custom providers. The page says which file it
// is writing, because these are the same files a pi user edits by hand and the
// two must not surprise each other.
//
// Codex is the odd one out: its sign-in is an OAuth flow pi only exposes through
// `/login` in its own interactive TUI, with no RPC command and no headless CLI
// behind it. So the button does not sign anyone in — it asks the window for a
// terminal already running the flow. Signing *out* is only dropping the stored
// credential, which is ours to do.
SettingsPage {
    id: page

    SettingsSection { text: qsTr("Credentials"); first: true }

    SettingsNote {
        text: qsTr("Keys are stored in pi's own %1, created 0600, and custom "
                   + "servers in models.json beside it. Both files stay editable "
                   + "by hand; nothing here rewrites what it did not set.")
              .arg(Settings.auth_path)
    }

    // ---- OpenRouter ----------------------------------------------------------

    SettingsGroup { text: "OpenRouter" }

    SettingRow {
        title: qsTr("API Key")
        description: qsTr("Billed from your OpenRouter credits. Pasted here it is "
                          + "written to auth.json, which pi prefers over the "
                          + "environment variable.")

        SettingsField {
            id: openrouterKey
            secret: true
            placeholderText: "sk-or-v1-…"
            onAccepted: page.saveOpenrouter()
        }
    }

    SettingsActions {
        SettingsChip { text: qsTr("Save"); onClicked: page.saveOpenrouter() }
        SettingsChip {
            text: qsTr("Forget")
            enabled: Settings.openrouter_stored
            onClicked: Settings.forget_openrouter_key()
        }
    }

    SettingsNote {
        text: page.openrouterError !== "" ? page.openrouterError
                                          : Settings.openrouter_status
        color: page.openrouterError !== "" ? Theme.chat_colors.error
                                           : Theme.chat_colors.dim
    }

    property string openrouterError: ""

    function saveOpenrouter() {
        openrouterError = Settings.save_openrouter_key(openrouterKey.text)
        // Cleared rather than left holding the key: it is stored now, and a
        // password field that stays full invites a second save of something
        // already saved.
        if (openrouterError === "")
            openrouterKey.clear()
    }

    // ---- Local models --------------------------------------------------------

    SettingsGroup { text: qsTr("Local Models") }

    SettingRow {
        title: qsTr("Provider ID")
        description: qsTr("The name this server appears under in the model picker.")

        SettingsField {
            id: localId
            placeholderText: "locallm"
            text: Settings.local_id
        }
    }

    SettingRow {
        title: qsTr("Server URL")
        description: qsTr("An OpenAI-compatible endpoint — llama.cpp, Ollama, "
                          + "LM Studio, vLLM. Include the /v1.")

        SettingsField {
            id: localUrl
            placeholderText: "http://localhost:8080/v1"
            text: Settings.local_url
        }
    }

    SettingRow {
        title: qsTr("Models")
        description: qsTr("Model ids the server exposes, comma separated. Anything "
                          + "else already configured for these models is kept.")

        SettingsField {
            id: localModels
            placeholderText: "qwen2.5-coder:7b, llama3.1:8b"
            text: Settings.local_models
        }
    }

    SettingsActions {
        SettingsChip {
            text: qsTr("Save")
            onClicked: page.localError = Settings.save_local_provider(
                localId.text, localUrl.text, localModels.text)
        }
        SettingsChip {
            text: qsTr("Remove")
            enabled: localId.text.trim() !== ""
            onClicked: {
                Settings.remove_local_provider(localId.text)
                page.localError = ""
            }
        }
    }

    SettingsNote {
        text: page.localError !== "" ? page.localError : Settings.local_status
        color: page.localError !== "" ? Theme.chat_colors.error : Theme.chat_colors.dim
    }

    property string localError: ""

    // ---- Codex ---------------------------------------------------------------

    SettingsGroup { text: qsTr("ChatGPT (Codex)") }

    SettingRow {
        title: qsTr("ChatGPT Plan")
        description: qsTr("Uses a ChatGPT Plus or Pro subscription. The sign-in is "
                          + "pi's own OAuth flow, so this opens a terminal running "
                          + "it; the token it stores is refreshed by pi from then on.")

        Row {
            spacing: 6
            SettingsChip {
                text: qsTr("Sign in…")
                onClicked: Settings.request_codex_signin()
            }
            SettingsChip {
                text: qsTr("Sign out")
                enabled: Settings.codex_signed_in
                onClicked: Settings.sign_out_codex()
            }
        }
    }

    SettingsNote { text: Settings.codex_status }
}
