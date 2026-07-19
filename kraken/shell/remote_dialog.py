"""Dialog for adding or editing a remote (SSH) workspace.

Captures a full SSH connection — hostname, user, port, identity file — plus the
project path on the remote host, and a Test Connection button that verifies it
before committing. Saved connections become reusable host profiles; the Host
selector also offers aliases discovered in ``~/.ssh/config`` to prefill the
form.
"""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMessageBox,
    QPushButton,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from kraken.agent import remote
from kraken.shell.async_run import run_command_async


class RemoteWorkspaceDialog(QDialog):
    """Collects an SshHost + remote path. On accept, saves the host profile and
    registers the remote workspace; the resulting anchor key is exposed as
    ``anchor``."""

    def __init__(self, parent: QWidget | None = None, anchor: str | None = None):
        super().__init__(parent)
        self._edit_anchor = anchor
        self.anchor: str | None = None
        self.setWindowTitle("Edit Remote Workspace" if anchor else "Add Remote Workspace")
        self.setMinimumWidth(440)

        self._host_picker = QComboBox()
        self._name = QLineEdit()
        self._name.setPlaceholderText("A label for this host, e.g. gpu-box")
        self._hostname = QLineEdit()
        self._hostname.setPlaceholderText("hostname or IP (or an ssh_config alias)")
        self._user = QLineEdit()
        self._port = QSpinBox()
        self._port.setRange(1, 65535)
        self._port.setValue(22)
        self._identity = QLineEdit()
        self._identity.setPlaceholderText("optional — defaults to your ssh keys")
        browse = QPushButton("Browse…")
        browse.clicked.connect(self._browse_identity)
        identity_row = QWidget()
        identity_layout = QHBoxLayout(identity_row)
        identity_layout.setContentsMargins(0, 0, 0, 0)
        identity_layout.addWidget(self._identity, stretch=1)
        identity_layout.addWidget(browse)
        self._path = QLineEdit()
        self._path.setPlaceholderText("/absolute/path/to/project on the remote")

        form = QFormLayout()
        form.addRow("Host", self._host_picker)
        form.addRow("Name", self._name)
        form.addRow("Hostname", self._hostname)
        form.addRow("User", self._user)
        form.addRow("Port", self._port)
        form.addRow("Identity file", identity_row)
        form.addRow("Remote path", self._path)

        self._status = QLabel("")
        self._status.setWordWrap(True)

        self._test_button = QPushButton("Test Connection")
        self._test_button.clicked.connect(self._test_connection)

        buttons = QDialogButtonBox(
            QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel
        )
        buttons.accepted.connect(self._on_accept)
        buttons.rejected.connect(self.reject)

        bottom = QHBoxLayout()
        bottom.addWidget(self._test_button)
        bottom.addStretch(1)
        bottom.addWidget(buttons)

        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(self._status)
        layout.addLayout(bottom)

        self._populate_host_picker()
        self._host_picker.currentIndexChanged.connect(self._on_host_picked)
        if anchor is not None:
            self._load_existing(anchor)

    # ---- Host picker -----------------------------------------------------

    def _populate_host_picker(self) -> None:
        """Fill the Host dropdown: a blank 'New host…' entry, then saved host
        profiles, then any aliases from ~/.ssh/config not already saved."""
        self._host_picker.addItem("New host…", None)
        saved = remote.load_hosts()
        for host in saved.values():
            self._host_picker.addItem(
                f"{host.host_id}  —  {host.destination}",
                {
                    "name": host.host_id,
                    "hostname": host.hostname,
                    "user": host.user,
                    "port": host.port,
                    "identity": host.identity or "",
                },
            )
        for entry in remote.parse_ssh_config():
            if entry["alias"] in saved:
                continue
            self._host_picker.addItem(
                f"{entry['alias']}  —  (ssh config)",
                {
                    "name": entry["alias"],
                    "hostname": entry["hostname"],
                    "user": entry["user"],
                    "port": entry["port"],
                    "identity": entry["identity"] or "",
                },
            )

    def _on_host_picked(self, _index: int) -> None:
        data = self._host_picker.currentData()
        if not data:
            return
        self._name.setText(data["name"])
        self._hostname.setText(data["hostname"])
        self._user.setText(data["user"])
        self._port.setValue(int(data["port"]))
        self._identity.setText(data["identity"])

    def _load_existing(self, anchor: str) -> None:
        target = remote.resolve(anchor)
        if target is None:
            return
        self._name.setText(target.host.host_id)
        self._hostname.setText(target.host.hostname)
        self._user.setText(target.host.user)
        self._port.setValue(target.host.port)
        self._identity.setText(target.host.identity or "")
        self._path.setText(target.path)

    def _browse_identity(self) -> None:
        path, _ = QFileDialog.getOpenFileName(self, "Select SSH identity file")
        if path:
            self._identity.setText(path)

    # ---- Build / validate ------------------------------------------------

    def _build_host(self) -> remote.SshHost | None:
        name = self._name.text().strip()
        hostname = self._hostname.text().strip()
        if not name or not hostname:
            self._status.setText("Name and hostname are required.")
            return None
        return remote.SshHost(
            host_id=name,
            hostname=hostname,
            user=self._user.text().strip(),
            port=self._port.value(),
            identity=self._identity.text().strip() or None,
        )

    def _test_connection(self) -> None:
        host = self._build_host()
        if host is None:
            return
        self._status.setText("Testing…")
        # Run the ssh probe off the GUI thread so the dialog stays responsive;
        # disable the button meanwhile so tests can't stack up.
        self._test_button.setEnabled(False)
        argv = ["ssh", *host.ssh_base_args(), host.destination, "true"]
        destination = host.destination
        run_command_async(
            argv,
            lambda result: self._on_test_result(result, destination),
            self,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=20,
        )

    def _on_test_result(self, result, destination: str) -> None:
        self._test_button.setEnabled(True)
        if result is None:
            self._status.setText("✗ Connection failed (timed out or ssh unavailable).")
        elif result.returncode == 0:
            self._status.setText(f"✓ Connected to {destination}.")
        else:
            self._status.setText(
                f"✗ Failed: {result.stderr.strip() or 'ssh exited ' + str(result.returncode)}"
            )

    def _on_accept(self) -> None:
        host = self._build_host()
        if host is None:
            return
        path = self._path.text().strip()
        if not path:
            self._status.setText("A remote path is required.")
            return
        remote.save_host(host)
        if self._edit_anchor is not None:
            # Editing in place: keep the same anchor, just update its path and
            # host reference.
            remotes = remote.load_remotes()
            entry = remotes.get(self._edit_anchor, {})
            entry["host_id"] = host.host_id
            entry["path"] = path
            remotes[self._edit_anchor] = entry
            from kraken.agent import config

            config.save_state(remotes=remotes)
            self.anchor = self._edit_anchor
        else:
            self.anchor = remote.add_remote_workspace(host.host_id, path)
        self.accept()
