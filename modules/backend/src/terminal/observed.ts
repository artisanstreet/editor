import type { TerminalSession } from "@artisan/protocol";

/**
 * Adopts the shells an engine runs underneath into durable terminal sessions.
 *
 * Artisan is a harness over another harness: when the engine runs `npm run dev`
 * through its own `Bash`/`PowerShell` tool, the normalizer already sees it and
 * emits a `terminal_activity` observation. That observation only ever became a
 * transcript row, so the Terminals card — which reads `terminal_sessions` —
 * stayed empty no matter what the engine was running.
 *
 * These sessions carry no PTY. Nothing here spawns a process: the command has
 * already run inside the engine's own harness, and this records what was seen.
 * So there is no pid, and write/resize/kill have nothing to act on. The card
 * and its tail viewer only ever read, which is exactly what an observed session
 * can honestly answer.
 *
 * Ownership is `agent` rather than a new kind, and that is not a shortcut: the
 * agent genuinely ran this command during this run. Reusing it keeps the
 * existing owner constraint intact and needs no migration.
 */

/** Identifies the run whose engine was observed running the command. */
export interface ObservedTerminalOwner {
	readonly agent_id: string;
	readonly run_id: string;
}

export interface ObservedTerminalContext {
	readonly observed_at: string;
	readonly owner: ObservedTerminalOwner;
	readonly thread_id: string;
	/** The root the engine runs in; an observed command has no cwd of its own. */
	readonly working_directory: string;
	readonly workspace_id: string;
}

/** The observation fields this adoption reads, named locally to avoid an engine import. */
export interface ObservedTerminalActivity {
	readonly activity_id: string;
	readonly command?: string | undefined;
	readonly exit_code?: number | undefined;
	/** The interpreter the provider handed the command to, when it names one. */
	readonly shell?: string | undefined;
	readonly state: "started" | "output" | "completed" | "failed";
}

/**
 * A terminal a person never opened still needs a stable identity across the
 * start, output, and completion frames of one command, so it is keyed on the
 * engine's own activity id rather than on the frame that carried it.
 */
export const ObservedTerminalId = (activity_id: string): string => `observed_${activity_id}`;

/**
 * Stands in for an interpreter the provider declined to name.
 *
 * Codex reports a command execution without saying what ran it, and the text
 * alone cannot be read backwards into a shell. Naming one anyway would be a
 * guess printed as a fact, so the row says only what is known: this ran in a
 * shell.
 */
export const unknown_observed_shell = "shell";

/**
 * Interpreters recognised as the leading program of an observed command.
 *
 * Only used to decide whether a command's first token is a shell being invoked
 * or the program the shell was told to run — a command starting with `git` is
 * the second, and must not be mistaken for the first.
 */
const known_shells = new Set([
	"ash",
	"bash",
	"cmd",
	"dash",
	"elvish",
	"fish",
	"ksh",
	"nu",
	"powershell",
	"pwsh",
	"sh",
	"xonsh",
	"zsh",
]);

/** The flags a shell uses to say "the next argument is the script". */
const shell_script_flags = new Set(["-c", "-lc", "-command", "/c", "-nologo"]);

/** Splits on whitespace while keeping quoted runs — paths have spaces in them. */
const tokenize = (command: string): ReadonlyArray<string> =>
	(command.match(/"[^"]*"|'[^']*'|\S+/gu) ?? []).map((token) =>
		/^(["']).*\1$/su.test(token) ? token.slice(1, -1) : token,
	);

/** The program name a path or filename resolves to, lowercased for matching. */
const program_name = (token: string) =>
	(token.split(/[\\/]/u).at(-1) ?? token).replace(/\.(?:bat|cmd|exe|ps1)$/iu, "").toLowerCase();

/**
 * Splits an observed command into the interpreter that ran it and the command
 * it was given.
 *
 * The executable is the shell rather than the command's first token: a row is
 * titled by what is running it and identified by what it is running, so the two
 * halves land on opposite sides of the row instead of the first word doing both
 * jobs badly.
 *
 * Two providers, two shapes. Claude names the shell out of band, because its
 * command is the bare text its tool was given. Codex names no shell but hands
 * over the whole invocation — `"…/pwsh.exe" -Command '…'` — so the interpreter
 * is already there as the leading program and only needs reading. Neither case
 * infers a shell from the command's syntax: almost everything an agent runs is
 * `npm test` or `git status`, which is identical in every shell, and the few
 * commands that would betray one are not worth guessing wrong about.
 */
export const SplitObservedCommand = (
	command: string,
	shell?: string,
): { readonly args: ReadonlyArray<string>; readonly executable: string } => {
	const declared = shell?.trim();
	const tokens = tokenize(command);
	if (declared) return { args: tokens, executable: declared };

	const [leading, ...rest] = tokens;
	if (leading === undefined || !known_shells.has(program_name(leading))) {
		return { args: tokens, executable: unknown_observed_shell };
	}

	/**
	 * Everything after the script flag is the command as it was written. The
	 * flags themselves belong to the invocation, not to what was run, and a row
	 * reading `-Command 'git status'` buries the part worth seeing.
	 */
	const script = rest.findIndex((token) => shell_script_flags.has(token.toLowerCase()));

	return {
		args: script === -1 ? rest : rest.slice(script + 1),
		executable: program_name(leading),
	};
};

/** Default geometry: an observed command has no terminal to have been sized by. */
const observed_cols = 80;
const observed_rows = 24;

const ObservedState = (
	state: ObservedTerminalActivity["state"],
): TerminalSession["state"] | undefined => {
	switch (state) {
		case "started":
			return "active";
		case "completed":
			return "closed";
		case "failed":
			return "failed";
		/** Output belongs to the session the start frame already opened. */
		case "output":
			return undefined;
	}
};

/**
 * Projects one observation onto a durable session, or reports that it carries
 * no session change.
 *
 * A command with no text cannot be adopted: the card identifies a row by its
 * command line, and a row that renders as an empty string is worse than no row.
 */
export const AdoptObservedTerminalActivity = (
	activity: ObservedTerminalActivity,
	context: ObservedTerminalContext,
): TerminalSession | undefined => {
	const state = ObservedState(activity.state);
	if (state === undefined) return undefined;
	const command = activity.command?.trim();
	if (command === undefined || command.length === 0) return undefined;
	const { args, executable } = SplitObservedCommand(command, activity.shell);
	const closed = state !== "active";

	return {
		args,
		cols: observed_cols,
		created_at: context.observed_at,
		executable,
		generation: 1,
		ownership: {
			agent_id: context.owner.agent_id,
			kind: "agent",
			run_id: context.owner.run_id,
		},
		rows: observed_rows,
		state,
		terminal_id: ObservedTerminalId(activity.activity_id),
		thread_id: context.thread_id,
		updated_at: context.observed_at,
		workspace_id: context.workspace_id,
		working_directory: context.working_directory,
		...(closed ? { closed_at: context.observed_at, exit_reason: "exited" as const } : {}),
		...(activity.exit_code === undefined ? {} : { exit_code: activity.exit_code }),
	} satisfies TerminalSession;
};
