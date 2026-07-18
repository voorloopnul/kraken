/**
 * Kraken SSH remote-execution extension.
 *
 * Routes pi's built-in read / write / edit / bash tools (and user `!` commands)
 * to a remote machine over SSH, while pi itself keeps running locally. This is
 * pi's "run on host, route tool execution into an isolated environment" pattern
 * (docs/containerization.md), specialised for a plain SSH host — the only thing
 * needed on the remote is a POSIX shell plus coreutils.
 *
 * It is a hardened fork of pi's examples/extensions/ssh.ts. Differences:
 *   - Connection comes from the KRAKEN_SSH env var (JSON), so it can carry a
 *     port, identity file, and ssh multiplexing options — things the stock
 *     example's `--ssh user@host` flag cannot express.
 *   - All ssh invocations reuse one multiplexed connection (ControlMaster),
 *     configured by Kraken in the baseArgs.
 *   - Robust local<->remote path mapping (relative paths resolve against the
 *     remote workspace path; absolute paths already outside the local anchor
 *     are passed through untouched).
 *   - bash runs through a remote login shell so PATH (nvm, ~/.local/bin, …) is
 *     populated the way an interactive session would have it.
 *
 * When KRAKEN_SSH is unset the extension is inert: it registers nothing and the
 * built-in local tools are used as normal. That keeps it harmless to load.
 */

import { spawn } from "node:child_process";
import * as path from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import {
	type BashOperations,
	createBashTool,
	createEditTool,
	createReadTool,
	createWriteTool,
	type EditOperations,
	type ReadOperations,
	type WriteOperations,
} from "@earendil-works/pi-coding-agent";

interface SshConfig {
	destination: string;
	baseArgs: string[];
	remotePath: string;
}

function loadConfig(): SshConfig | null {
	const raw = process.env.KRAKEN_SSH;
	if (!raw) return null;
	try {
		const parsed = JSON.parse(raw);
		if (!parsed.destination) return null;
		return {
			destination: parsed.destination,
			baseArgs: Array.isArray(parsed.baseArgs) ? parsed.baseArgs : [],
			remotePath: parsed.remotePath || ".",
		};
	} catch {
		return null;
	}
}

/** POSIX single-quote a string for safe embedding in a remote shell command. */
function shq(s: string): string {
	return `'${s.replace(/'/g, `'\\''`)}'`;
}

function sshExec(cfg: SshConfig, command: string): Promise<Buffer> {
	return new Promise((resolve, reject) => {
		const child = spawn("ssh", [...cfg.baseArgs, cfg.destination, command], {
			stdio: ["ignore", "pipe", "pipe"],
		});
		const out: Buffer[] = [];
		const err: Buffer[] = [];
		child.stdout.on("data", (d) => out.push(d));
		child.stderr.on("data", (d) => err.push(d));
		child.on("error", reject);
		child.on("close", (code) => {
			if (code !== 0) {
				reject(new Error(`SSH failed (${code}): ${Buffer.concat(err).toString()}`));
			} else {
				resolve(Buffer.concat(out));
			}
		});
	});
}

/**
 * Translate a path pi resolved against the local anchor into the equivalent
 * path on the remote. pi resolves relative paths against `localCwd` before
 * handing them to an operation, so most paths arrive absolute and under
 * `localCwd`; those are re-rooted at `remotePath`. A path already outside the
 * anchor is assumed to be an explicit remote absolute path and passed through.
 */
function makeToRemote(localCwd: string, remotePath: string): (p: string) => string {
	return (p: string) => {
		if (!path.isAbsolute(p)) {
			return path.posix.join(remotePath, p.split(path.sep).join("/"));
		}
		if (p === localCwd) return remotePath;
		const rel = path.relative(localCwd, p);
		if (!rel.startsWith("..") && !path.isAbsolute(rel)) {
			return path.posix.join(remotePath, rel.split(path.sep).join("/"));
		}
		return p;
	};
}

function remoteReadOps(cfg: SshConfig, toRemote: (p: string) => string): ReadOperations {
	return {
		readFile: (p) => sshExec(cfg, `cat ${shq(toRemote(p))}`),
		access: (p) => sshExec(cfg, `test -r ${shq(toRemote(p))}`).then(() => {}),
		detectImageMimeType: async (p) => {
			try {
				const m = (await sshExec(cfg, `file --mime-type -b ${shq(toRemote(p))}`))
					.toString()
					.trim();
				return ["image/jpeg", "image/png", "image/gif", "image/webp"].includes(m) ? m : null;
			} catch {
				return null;
			}
		},
	};
}

function remoteWriteOps(cfg: SshConfig, toRemote: (p: string) => string): WriteOperations {
	return {
		writeFile: async (p, content) => {
			const b64 = Buffer.from(content).toString("base64");
			// base64 round-trip keeps arbitrary bytes intact through the shell.
			await sshExec(cfg, `printf %s ${shq(b64)} | base64 -d > ${shq(toRemote(p))}`);
		},
		mkdir: (dir) => sshExec(cfg, `mkdir -p ${shq(toRemote(dir))}`).then(() => {}),
	};
}

function remoteEditOps(cfg: SshConfig, toRemote: (p: string) => string): EditOperations {
	const r = remoteReadOps(cfg, toRemote);
	const w = remoteWriteOps(cfg, toRemote);
	return { readFile: r.readFile, access: r.access, writeFile: w.writeFile };
}

function remoteBashOps(cfg: SshConfig, toRemote: (p: string) => string): BashOperations {
	return {
		exec: (command, cwd, { onData, signal, timeout }) =>
			new Promise((resolve, reject) => {
				// Run inside the remote cwd, through a login shell so PATH is set.
				const inner = `cd ${shq(toRemote(cwd))} && ${command}`;
				const remote = `bash -lc ${shq(inner)}`;
				const child = spawn("ssh", [...cfg.baseArgs, cfg.destination, remote], {
					stdio: ["ignore", "pipe", "pipe"],
				});
				let timedOut = false;
				const timer = timeout
					? setTimeout(() => {
							timedOut = true;
							child.kill();
						}, timeout * 1000)
					: undefined;
				child.stdout.on("data", onData);
				child.stderr.on("data", onData);
				child.on("error", (e) => {
					if (timer) clearTimeout(timer);
					reject(e);
				});
				const onAbort = () => child.kill();
				signal?.addEventListener("abort", onAbort, { once: true });
				child.on("close", (code) => {
					if (timer) clearTimeout(timer);
					signal?.removeEventListener("abort", onAbort);
					if (signal?.aborted) reject(new Error("aborted"));
					else if (timedOut) reject(new Error(`timeout:${timeout}`));
					else resolve({ exitCode: code });
				});
			}),
	};
}

export default function (pi: ExtensionAPI) {
	const cfg = loadConfig();
	if (!cfg) return; // No remote configured: leave the built-in tools in place.

	const localCwd = process.cwd();
	const toRemote = makeToRemote(localCwd, cfg.remotePath);

	const read = createReadTool(localCwd);
	const write = createWriteTool(localCwd);
	const edit = createEditTool(localCwd);
	const bash = createBashTool(localCwd);

	pi.registerTool({
		...read,
		async execute(id, params, signal, onUpdate) {
			const tool = createReadTool(localCwd, { operations: remoteReadOps(cfg, toRemote) });
			return tool.execute(id, params, signal, onUpdate);
		},
	});
	pi.registerTool({
		...write,
		async execute(id, params, signal, onUpdate) {
			const tool = createWriteTool(localCwd, { operations: remoteWriteOps(cfg, toRemote) });
			return tool.execute(id, params, signal, onUpdate);
		},
	});
	pi.registerTool({
		...edit,
		async execute(id, params, signal, onUpdate) {
			const tool = createEditTool(localCwd, { operations: remoteEditOps(cfg, toRemote) });
			return tool.execute(id, params, signal, onUpdate);
		},
	});
	pi.registerTool({
		...bash,
		async execute(id, params, signal, onUpdate) {
			const tool = createBashTool(localCwd, { operations: remoteBashOps(cfg, toRemote) });
			return tool.execute(id, params, signal, onUpdate);
		},
	});

	// User `!` commands run on the remote too.
	pi.on("user_bash", () => ({ operations: remoteBashOps(cfg, toRemote) }));

	// Tell the model its working directory is the remote path, not the local
	// anchor pi actually runs in.
	pi.on("before_agent_start", (event) => {
		const modified = event.systemPrompt.replace(
			`Current working directory: ${localCwd}`,
			`Current working directory: ${cfg.remotePath} (remote, via SSH: ${cfg.destination})`,
		);
		return { systemPrompt: modified };
	});

	pi.on("session_start", async (_event, ctx) => {
		ctx.ui.setStatus("ssh", ctx.ui.theme.fg("accent", `SSH: ${cfg.destination}:${cfg.remotePath}`));
	});
}
