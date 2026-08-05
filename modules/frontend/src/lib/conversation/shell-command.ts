/** One recognised shell wrapper: how its executable is named and how it takes a command. */
interface ShellWrapper {
	readonly flags: ReadonlySet<string>;
	readonly names: ReadonlySet<string>;
}

const shell_wrappers: ReadonlyArray<ShellWrapper> = [
	{ flags: new Set(["-command", "-c"]), names: new Set(["pwsh", "powershell"]) },
	{ flags: new Set(["-c", "-lc", "-ic", "-lic"]), names: new Set(["bash", "sh", "zsh", "dash"]) },
	{ flags: new Set(["/c", "/k"]), names: new Set(["cmd"]) },
];

/** Splits a leading argument, honouring the quotes a path with spaces needs. */
const take_argument = (input: string) => {
	const text = input.trimStart();
	const quote = text.startsWith('"') ? '"' : text.startsWith("'") ? "'" : undefined;

	if (quote !== undefined) {
		const close = text.indexOf(quote, 1);
		if (close !== -1) return { rest: text.slice(close + 1), value: text.slice(1, close) };
	}

	const break_at = text.search(/\s/);

	return break_at === -1
		? { rest: "", value: text }
		: { rest: text.slice(break_at), value: text.slice(0, break_at) };
};

/** The executable's own name, without its directory or extension. */
const executable_name = (path: string) => {
	const separator = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
	const file = separator === -1 ? path : path.slice(separator + 1);

	return file.toLowerCase().replace(/\.exe$/, "");
};

/** Drops one matching pair of surrounding quotes, which a shell would have eaten. */
const unquote = (text: string) => {
	const first = text.at(0);

	return (first === '"' || first === "'") && text.length > 1 && text.endsWith(first)
		? text.slice(1, -1)
		: text;
};

/**
 * The command a reader means, recovered from the invocation an engine reports.
 *
 * A run arrives as the full argv it was spawned with, which on Windows begins
 * with an absolute path into `WindowsApps` before `-Command` and the actual
 * work. In a truncated row every visible character would be that path, so the
 * line would say less than the word it replaced. The wrapper is presentation
 * noise; the raw invocation stays intact in provenance.
 *
 * Anything not recognised is returned as it came, because a command this does
 * not understand is still more useful whole than guessed at.
 */
export const PresentShellCommand = (command: string): string => {
	const collapsed = command.replaceAll(/\s+/g, " ").trim();
	const executable = take_argument(collapsed);
	const name = executable_name(executable.value);
	const wrapper = shell_wrappers.find((candidate) => candidate.names.has(name));

	if (wrapper === undefined) return collapsed;

	const flag = take_argument(executable.rest);
	if (!wrapper.flags.has(flag.value.toLowerCase())) return collapsed;

	const body = unquote(flag.rest.trim());

	return body.length === 0 ? collapsed : body;
};
