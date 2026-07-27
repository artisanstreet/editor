import { Schema } from "effect";

import { NpmCleanupPlan, type BootstrapInvocation } from "./contract";

const QuoteCommandArgument = (argument: string) =>
	/^[A-Za-z0-9_./:\\-]+$/.test(argument) ? argument : `"${argument.replaceAll('"', '\\"')}"`;

/** Builds the exact argv and human-readable fallback for the bootstrap's own package. */
export const MakeNpmCleanupPlan = (invocation: BootstrapInvocation): NpmCleanupPlan =>
	Schema.decodeUnknownSync(NpmCleanupPlan)({
		executable: invocation.npm_executable,
		argv: ["uninstall", "--global", "--prefix", invocation.npm_prefix, invocation.package_name],
		bootstrap_pid: invocation.bootstrap_pid,
		manual_command: [
			invocation.npm_executable,
			"uninstall",
			"--global",
			"--prefix",
			invocation.npm_prefix,
			invocation.package_name,
		]
			.map(QuoteCommandArgument)
			.join(" "),
	});
