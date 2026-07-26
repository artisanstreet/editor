#!/usr/bin/env node
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import {
	NodeChildProcessSpawner,
	NodeFileSystem,
	NodePath,
	NodeRuntime,
	NodeStdio,
	NodeTerminal,
} from "@effect/platform-node-shared";
import { Console, Effect, Layer, Option, Stream } from "effect";
import { Command, Flag, Prompt } from "effect/unstable/cli";

import {
	BrowserOpener,
	CliPlatform,
	ForgeAutostart,
	make_browser_opener_layer,
	make_cli_platform_layer,
	make_windows_autostart_layer,
} from "./adapters";
import { ForgeLifecycle, make_forge_lifecycle_layer } from "./lifecycle";
import { ForgeControlLive } from "./node-control";
import { make_node_forge_launcher_layer } from "./node-launcher";
import { make_node_profile_store_layer } from "./node-profile-store";
import { ForgeProfileStore } from "./profile";
import { ForgeOperations, make_forge_operations_layer } from "./operations";

/**
 * Declarative command surface. Runtime composition is deliberately exported by
 * the module, so packaging can provide Forge adapters without this entrypoint
 * importing a concurrently renamed host package.
 */
const profile = Flag.string("profile").pipe(
	Flag.withDefault("default"),
	Flag.withDescription("Forge profile name"),
);
const json = Flag.boolean("json").pipe(Flag.withDescription("Emit deterministic JSON"));
const foreground = Flag.boolean("foreground").pipe(
	Flag.withDescription("Run attached to this terminal"),
);
const project_root = Flag.string("project-root").pipe(
	Flag.atLeast(0),
	Flag.withDescription("Allowed project root; repeat as needed"),
);
const mode = Flag.choice("mode", ["local", "headless"] as const).pipe(
	Flag.withDefault("local"),
	Flag.withDescription("Local UI or headless service mode"),
);
const data_root = Flag.optional(
	Flag.string("data-root").pipe(Flag.withDescription("Forge data directory")),
);
const listen_port = Flag.optional(
	Flag.integer("listen-port").pipe(Flag.withDescription("Loopback listen port (0 selects one)")),
);
const autostart = Flag.boolean("autostart").pipe(
	Flag.withDescription("Enable current-user Forge autostart"),
);
const follow = Flag.boolean("follow").pipe(Flag.withDescription("Stream appended log content"));
const lines = Flag.integer("lines").pipe(
	Flag.withDefault(200),
	Flag.withDescription("Number of log lines (1–10000)"),
);
const origin = Flag.optional(
	Flag.string("origin").pipe(Flag.withDescription("Explicit local browser origin")),
);

export const AeCommand = Command.make("ae").pipe(
	Command.withSubcommands([
		Command.make(
			"setup",
			{ autostart, data_root, listen_port, mode, profile, project_root },
			(input) =>
				Effect.gen(function* () {
					const store = yield* ForgeProfileStore;
					const platform = yield* CliPlatform;
					const roots =
						input.project_root.length > 0
							? input.project_root
							: [yield* PromptForProjectRoot()];
					yield* store.Ensure(input.profile, {
						data_root: Option.getOrElse(
							input.data_root,
							() => `${platform.home}/profiles/${input.profile}/data`,
						),
						listen_host: "127.0.0.1",
						listen_port: Option.getOrElse(input.listen_port, () => 0),
						mode: input.mode,
						project_roots: roots as readonly [string, ...string[]],
						version: 1,
					});
					if (input.autostart)
						yield* (yield* ForgeAutostart).Configure({
							enabled: true,
							profile: input.profile,
						});
					yield* Console.log(`Configured Forge profile ${input.profile}`);
				}),
		).pipe(Command.withDescription("Create or update a loopback-only Forge profile")),
		Command.make("start", { foreground, profile }, (input) =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Start(input.profile, input.foreground);
			}),
		).pipe(Command.withDescription("Start Forge in the background")),
		Command.make("stop", { profile }, (input) =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Stop(input.profile);
			}),
		).pipe(Command.withDescription("Stop the named Forge instance")),
		Command.make("restart", { foreground, profile }, (input) =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Restart(input.profile, input.foreground);
			}),
		).pipe(Command.withDescription("Restart the named Forge instance")),
		Command.make("status", { json, profile }, (input) =>
			Effect.gen(function* () {
				const result = yield* (yield* ForgeLifecycle).Status(input.profile);
				yield* Console.log(input.json ? JSON.stringify(result) : result.state);
			}),
		).pipe(Command.withDescription("Show Forge runtime status")),
		Command.make("logs", { follow, lines, profile }, (input) =>
			Effect.gen(function* () {
				const operations = yield* ForgeOperations;
				const recent = yield* operations.ReadLogs(input.profile, input.lines);
				yield* Effect.forEach(recent, (line) => Console.log(line));
				if (input.follow)
					yield* operations
						.FollowLogs(input.profile)
						.pipe(Stream.runForEach((chunk) => Console.log(chunk)));
			}),
		).pipe(Command.withDescription("Show or follow Forge log output")),
		Command.make("doctor", { json, profile }, (input) =>
			Effect.gen(function* () {
				const result = yield* (yield* ForgeOperations).Doctor(input.profile);
				yield* Console.log(
					input.json
						? JSON.stringify(result)
						: result.checks
								.map((check) => `${check.state}: ${check.name} — ${check.detail}`)
								.join("\n"),
				);
			}),
		).pipe(Command.withDescription("Check local Forge prerequisites and status")),
		Command.make("open", { origin, profile }, (input) =>
			Effect.gen(function* () {
				const url = yield* (yield* ForgeLifecycle).Open(
					input.profile,
					Option.getOrUndefined(input.origin),
				);
				yield* (yield* BrowserOpener).Open(url);
			}),
		).pipe(Command.withDescription("Open a one-time local browser pairing link")),
	]),
);

/** Keeps Prompt in the stable CLI boundary for interactive setup composition. */
export const PromptForProjectRoot = () => Prompt.text({ message: "Project root" });

const NodePlatformLive = Layer.mergeAll(
	NodeFileSystem.layer,
	NodePath.layer,
	NodeStdio.layer,
	NodeTerminal.layer,
	NodeChildProcessSpawner.layer.pipe(
		Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
	),
);
const CliPlatformLive = make_cli_platform_layer().pipe(Layer.provide(NodePath.layer));

const ProfileStoreLive = make_node_profile_store_layer();
const ForgeLauncherLive = make_node_forge_launcher_layer.pipe(Layer.provide(ProfileStoreLive));
const ForgeLifecycleLive = make_forge_lifecycle_layer.pipe(
	Layer.provide(Layer.mergeAll(ProfileStoreLive, ForgeLauncherLive, ForgeControlLive)),
);
const ForgeOperationsLive = make_forge_operations_layer.pipe(
	Layer.provide(
		Layer.mergeAll(ProfileStoreLive, ForgeLifecycleLive, make_windows_autostart_layer()),
	),
);

const AeRuntimeLive = Layer.mergeAll(
	NodePlatformLive,
	ProfileStoreLive,
	ForgeLauncherLive,
	ForgeControlLive,
	ForgeLifecycleLive,
	ForgeOperationsLive,
	make_browser_opener_layer,
	make_windows_autostart_layer(),
).pipe(Layer.provideMerge(CliPlatformLive));

/** Runs only when this module is invoked as the `ae` executable, keeping imports test-safe. */
export const AeProgram = Command.run(AeCommand, { version: "0.1.0" }).pipe(
	Effect.provide(AeRuntimeLive),
);

/** Exact resolved paths keep import-only tests from running the bundled ae.js executable. */
export const IsAeDirectEntry = (argv_path = process.argv[1]) =>
	argv_path !== undefined && resolve(argv_path) === fileURLToPath(import.meta.url);

if (IsAeDirectEntry()) {
	NodeRuntime.runMain(AeProgram);
}
