import type { TerminalSession } from "@artisan/protocol";

/**
 * Keeps roughly the last window of a long-running log so an unbounded dev
 * server cannot grow the viewer's buffer forever. Matches the spirit of the
 * backend's bounded scrollback rather than any exact byte count.
 */
const OUTPUT_LIMIT_CHARS = 200_000;

/**
 * Matches CSI, OSC, DCS/SOS/PM/APC, and two-character escape sequences. The
 * viewer is a plain-text tail, not a terminal emulator, so every sequence is
 * dropped rather than interpreted.
 */
const ansi_pattern = new RegExp(
	[
		"\\u001b\\[[0-9:;<=>?]*[ !\"#$%&'()*+,\\-./]*[@-~]",
		"\\u001b\\][^\\u0007\\u001b]*(?:\\u0007|\\u001b\\\\)?",
		"\\u001b[PX^_][^\\u001b]*(?:\\u001b\\\\)?",
		"\\u001b[@-Z\\\\\\]^_]",
	].join("|"),
	"gu",
);

// oxlint-disable-next-line no-control-regex -- stripping raw control bytes is the entire point here
const control_pattern = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/gu;

/**
 * Resolves carriage returns the way a terminal renders them: the cursor
 * returns to column 0 and later text overwrites earlier text, so a progress
 * bar collapses to its final frame instead of stacking every update.
 */
function resolve_carriage_returns(line: string) {
	const segments = line.split("\r");
	let resolved = segments[0] ?? "";

	for (const segment of segments.slice(1)) {
		resolved =
			segment.length >= resolved.length ? segment : segment + resolved.slice(segment.length);
	}

	return resolved;
}

/** Renders raw accumulated PTY text as plain readable lines. */
export function present_terminal_output(raw: string) {
	return raw
		.replace(ansi_pattern, "")
		.replace(control_pattern, "")
		.split("\n")
		.map(resolve_carriage_returns)
		.join("\n");
}

/**
 * Accumulates raw bytes-as-text without stripping: escape sequences can split
 * across chunk boundaries, so sanitizing belongs to presentation over the
 * whole buffer, never to the chunk.
 */
export function append_terminal_output(existing: string, chunk: string) {
	const combined = existing + chunk;

	return combined.length > OUTPUT_LIMIT_CHARS
		? combined.slice(combined.length - OUTPUT_LIMIT_CHARS)
		: combined;
}

/**
 * What the terminal was told to run, without the interpreter that runs it.
 *
 * The row names the shell on its left, so repeating it here would spend the
 * scarce half of the row saying the same word twice.
 */
export function terminal_command_line(session: TerminalSession) {
	return session.args
		.map((argument) => (argument.includes(" ") ? `"${argument}"` : argument))
		.join(" ");
}

/**
 * Shells wear the names people call them, not the executables they ship as.
 * `pwsh` and `powershell` are one shell under two filenames, and `nu` is a
 * program name almost nobody would recognise written out.
 */
const shell_display_names: Readonly<Record<string, string>> = {
	ash: "Ash",
	bash: "Bash",
	cmd: "Command Prompt",
	dash: "Dash",
	elvish: "Elvish",
	fish: "Fish",
	ksh: "Ksh",
	nu: "Nushell",
	nushell: "Nushell",
	powershell: "PowerShell",
	pwsh: "PowerShell",
	sh: "Shell",
	shell: "Shell",
	xonsh: "Xonsh",
	zsh: "Zsh",
};

/**
 * The interpreter running the terminal, as its title.
 *
 * A shell nobody has a name for keeps its own program name rather than being
 * flattened into a generic word — an unfamiliar row that says `oil` is more
 * use than one that says "Shell".
 */
export function terminal_display_name(session: TerminalSession) {
	const program = session.executable.split(/[\\/]/u).at(-1) ?? session.executable;
	const bare = program.replace(/\.(?:bat|cmd|exe|ps1)$/iu, "");

	return shell_display_names[bare.toLowerCase()] ?? bare;
}

/** Live means the card shows it; exited terminals disappear like finished agents. */
export function is_live_terminal(session: TerminalSession) {
	return session.state === "opening" || session.state === "active";
}

/** Applies one lifecycle event: replaces the known session or appends a new one. */
export function apply_terminal_lifecycle(
	terminals: ReadonlyArray<TerminalSession>,
	next: TerminalSession,
): ReadonlyArray<TerminalSession> {
	return terminals.some((session) => session.terminal_id === next.terminal_id)
		? terminals.map((session) => (session.terminal_id === next.terminal_id ? next : session))
		: [...terminals, next];
}
