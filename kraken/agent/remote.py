"""Remote SSH workspaces: host profiles, connection targets, and helpers to
run commands on the remote over SSH.

Kraken's constraint is that ``pi`` always runs on the local machine — only SSH
is used to reach a remote host. A workspace that lives on a remote machine is
modelled in two levels:

* an **SSH host profile** (``SshHost``): a reusable connection — hostname, user,
  port, identity file — that several workspaces can share. Profiles can be
  imported from ``~/.ssh/config`` aliases.
* a **remote workspace**: a host profile (by id) plus a path on that host.

Each remote workspace also gets a local *anchor* directory under
``~/.kraken/remotes/``. The local ``pi`` process runs there, so it has a valid
cwd and a place to store its session files (which keeps the History pane
working unchanged); its tools operate on the remote path instead, via the
bundled ssh extension (see ``assets/ssh_remote.ts``). The anchor's absolute
path doubles as the workspace key, so the rest of the app stays path-keyed.

All SSH invocations share one multiplexed connection per host (ControlMaster),
so the many short commands the agent, terminal, and git panel issue don't each
re-authenticate.
"""

from __future__ import annotations

import json
import re
import shlex
from importlib.resources import files
from pathlib import Path

from dataclasses import dataclass

from kraken.agent import config

# ControlMaster sockets, one per user@host:port. Kept short so the resulting
# socket path stays under the ~104-char AF_UNIX limit.
_CONTROL_DIR = config.CONFIG_DIR / "ssh"
# Local anchor directories, one per remote workspace.
REMOTES_DIR = config.CONFIG_DIR / "remotes"
# Where the bundled pi ssh extension is unpacked for pi to load with `-e`.
_EXT_DIR = config.CONFIG_DIR / "ext"


def ssh_extension_path() -> str:
    """Unpack the bundled ssh routing extension to a stable on-disk path and
    return it, for `pi -e <path>`. Reads through importlib.resources so it
    works from a source checkout and from inside the packaged .pyz alike;
    rewritten on every call so it tracks the shipped version."""
    _EXT_DIR.mkdir(parents=True, exist_ok=True)
    dest = _EXT_DIR / "ssh_remote.ts"
    data = files("kraken.agent.assets").joinpath("ssh_remote.ts").read_bytes()
    # Rewrite only when the bundled version changed, so the common case (agent
    # starts on every session) is a cheap read rather than a disk write.
    try:
        if dest.read_bytes() == data:
            return str(dest)
    except OSError:
        pass
    dest.write_bytes(data)
    return str(dest)


# ---------------------------------------------------------------------------
# Host profiles + resolved targets
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class SshHost:
    """A reusable SSH connection profile."""

    host_id: str  # stable key and display alias
    hostname: str  # actual host to reach (may itself be an ssh_config alias)
    user: str = ""
    port: int = 22
    identity: str | None = None

    @classmethod
    def from_dict(cls, host_id: str, data: dict) -> "SshHost":
        return cls(
            host_id=host_id,
            hostname=data.get("hostname") or host_id,
            user=data.get("user", "") or "",
            port=int(data.get("port") or 22),
            identity=data.get("identity") or None,
        )

    def to_dict(self) -> dict:
        return {
            "hostname": self.hostname,
            "user": self.user,
            "port": self.port,
            "identity": self.identity,
        }

    @property
    def destination(self) -> str:
        """The ``user@host`` (or bare host) argument passed to ssh."""
        return f"{self.user}@{self.hostname}" if self.user else self.hostname

    def ssh_base_args(self) -> list[str]:
        """Common ssh options (everything but the destination and command):
        port, identity, and connection multiplexing."""
        _CONTROL_DIR.mkdir(parents=True, exist_ok=True)
        args = [
            "-p", str(self.port),
            "-o", "ControlMaster=auto",
            # %C is a short fixed-length hash of the connection tuple, so the
            # socket path stays well under the ~104-char AF_UNIX limit even for
            # long home dirs, usernames, or hostnames (a literal %r@%h:%p can
            # overflow it and make ssh disable multiplexing).
            "-o", f"ControlPath={_CONTROL_DIR}/cm-%C",
            "-o", "ControlPersist=120",
            # Key-based auth only: never block the UI on an interactive prompt.
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
        ]
        if self.identity:
            args += ["-i", str(Path(self.identity).expanduser())]
        return args


@dataclass(frozen=True)
class RemoteTarget:
    """A resolved remote workspace: a host profile plus a path on that host."""

    host: SshHost
    path: str

    def ssh_argv(self, remote_command: str, tty: bool = False) -> list[str]:
        """Full argv to run ``remote_command`` on the remote, from the
        workspace path. ``remote_command`` is a shell string."""
        argv = ["ssh"]
        if tty:
            argv.append("-t")
        argv += self.host.ssh_base_args()
        argv.append(self.host.destination)
        argv.append(f"cd {shlex.quote(self.path)} && {remote_command}")
        return argv

    def git_argv(self, git_args: list[str]) -> list[str]:
        """argv running ``git <git_args>`` in the workspace path on the
        remote. The whole remote command is one string ssh hands to the login
        shell, so each git argument is shell-quoted."""
        remote = "git " + " ".join(shlex.quote(a) for a in git_args)
        return self.ssh_argv(remote)

    def terminal_argv(self) -> list[str]:
        """argv for an interactive login shell on the remote, in the workspace
        path — what the terminal pane execs (with a PTY)."""
        return self.ssh_argv('exec "${SHELL:-/bin/bash}" -l', tty=True)

    def env_value(self) -> str:
        """JSON connection descriptor handed to the bundled pi ssh extension
        through the ``KRAKEN_SSH`` environment variable."""
        return json.dumps(
            {
                "destination": self.host.destination,
                "baseArgs": self.host.ssh_base_args(),
                "remotePath": self.path,
            }
        )


# ---------------------------------------------------------------------------
# State: host profiles and remote workspaces
# ---------------------------------------------------------------------------


def load_hosts() -> dict[str, SshHost]:
    raw = config.load_state().get("ssh_hosts", {})
    return {hid: SshHost.from_dict(hid, data) for hid, data in raw.items()}


def save_host(host: SshHost) -> None:
    hosts = load_hosts()
    hosts[host.host_id] = host
    config.save_state(ssh_hosts={hid: h.to_dict() for hid, h in hosts.items()})


def delete_host(host_id: str) -> None:
    hosts = load_hosts()
    if hosts.pop(host_id, None) is not None:
        config.save_state(
            ssh_hosts={hid: h.to_dict() for hid, h in hosts.items()}
        )


def load_remotes() -> dict[str, dict]:
    """Map of anchor path -> {"host_id", "path"} for every remote workspace."""
    return dict(config.load_state().get("remotes", {}))


def resolve(anchor: str) -> RemoteTarget | None:
    """The RemoteTarget for a workspace key, or None if it's a local folder or
    its host profile has gone missing."""
    entry = load_remotes().get(str(anchor))
    if entry is None:
        return None
    host = load_hosts().get(entry.get("host_id"))
    if host is None:
        return None
    return RemoteTarget(host=host, path=entry.get("path", "."))


def _slugify(text: str) -> str:
    slug = re.sub(r"[^0-9A-Za-z]+", "-", text).strip("-").lower()
    return slug or "remote"


def add_remote_workspace(host_id: str, path: str) -> str:
    """Register a remote workspace for ``host_id`` at ``path``, creating its
    local anchor directory. Returns the anchor's absolute path (the workspace
    key). Re-registering the same host+path returns the existing anchor."""
    remotes = load_remotes()
    for anchor, entry in remotes.items():
        if entry.get("host_id") == host_id and entry.get("path") == path:
            Path(anchor).mkdir(parents=True, exist_ok=True)
            return anchor

    # The slug is derived from host+path, so a leftover anchor dir with this
    # name is from this same workspace (removed earlier, its folder kept); reuse
    # it so a re-add resurfaces the stored sessions. Only bump the name when the
    # slug is an *active* key for a different workspace.
    base = f"{_slugify(host_id)}-{_slugify(path)}"
    anchor = REMOTES_DIR / base
    n = 2
    while str(anchor) in remotes:
        anchor = REMOTES_DIR / f"{base}-{n}"
        n += 1
    anchor.mkdir(parents=True, exist_ok=True)
    key = str(anchor)
    remotes[key] = {"host_id": host_id, "path": path}
    config.save_state(remotes=remotes)
    return key


def remove_remote_workspace(anchor: str) -> None:
    remotes = load_remotes()
    if remotes.pop(str(anchor), None) is not None:
        config.save_state(remotes=remotes)


# ---------------------------------------------------------------------------
# ~/.ssh/config import
# ---------------------------------------------------------------------------


def _split_option(line: str) -> tuple[str, str]:
    """Split an ssh_config line into (key, value). Options may be written as
    "Key value" or "Key=value"; keys are case-insensitive."""
    if "=" in line and (
        " " not in line.split("=", 1)[0].strip()
    ):
        key, _, value = line.partition("=")
    else:
        parts = line.split(None, 1)
        key = parts[0]
        value = parts[1] if len(parts) > 1 else ""
    return key.strip().lower(), value.strip()


def parse_ssh_config(path: Path | None = None) -> list[dict]:
    """Read ``~/.ssh/config`` and return one dict per concrete Host alias
    (wildcard patterns skipped), each with the alias plus any resolved
    hostname/user/port/identity. Best-effort: an unreadable file yields an
    empty list.

    An ``Host a b`` line may name several aliases; option lines that follow
    apply to every alias in that block until the next ``Host`` line.
    """
    path = path or Path.home() / ".ssh" / "config"
    try:
        lines = path.read_text().splitlines()
    except OSError:
        return []

    entries: list[dict] = []
    group: list[dict] = []  # aliases the current option lines apply to
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        key, value = _split_option(line)
        if key == "host":
            group = []
            for alias in value.split():
                if any(c in alias for c in "*?!"):
                    continue
                entry = {
                    "alias": alias,
                    "hostname": alias,
                    "user": "",
                    "port": 22,
                    "identity": None,
                }
                entries.append(entry)
                group.append(entry)
            continue
        for entry in group:
            if key == "hostname":
                entry["hostname"] = value
            elif key == "user":
                entry["user"] = value
            elif key == "port":
                try:
                    entry["port"] = int(value)
                except ValueError:
                    pass
            elif key == "identityfile":
                entry["identity"] = value
    return entries
