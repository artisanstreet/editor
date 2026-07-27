#!/usr/bin/env node

import { fileURLToPath } from "node:url";

import { NodeRuntime } from "@effect/platform-node-shared";
import { Console, Effect } from "effect";

import { BootstrapInvocation } from "./contract";
import {
	make_node_bootstrap_layer,
	ParseDetachedCleanup,
	ResolveNpmExecutable,
	ResolveNpmPrefix,
	RunDetachedCleanup,
} from "./node-runtime";
import { RunBootstrap } from "./workflow";

export { RunBootstrap } from "./workflow";

const entry_path = fileURLToPath(import.meta.url);

export const BootstrapProgram = Effect.gen(function* () {
	const route = yield* ParseDetachedCleanup(process.argv.slice(2));
	if (route._tag === "Cleanup") {
		yield* RunDetachedCleanup(route.plan);
		return 0;
	}

	const outcome = yield* RunBootstrap(
		BootstrapInvocation.make({
			argv: process.argv.slice(2),
			bootstrap_pid: process.pid,
			npm_executable: ResolveNpmExecutable(),
			npm_prefix: ResolveNpmPrefix(entry_path),
			package_name: "artisan-editor",
		}),
	).pipe(Effect.provide(make_node_bootstrap_layer(entry_path)));
	if (outcome.cleanup.state === "manual") {
		yield* Console.error(
			`Automatic bootstrap cleanup was unavailable. Run: ${outcome.cleanup.command}`,
		);
	}
	return outcome.exit_code;
});

NodeRuntime.runMain(BootstrapProgram, {
	disableErrorReporting: false,
});
