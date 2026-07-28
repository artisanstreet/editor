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
import { Command, Flag } from "effect/unstable/cli";
import { ListForgeInstances, ResolveInstanceRegistryRoot } from "@artisan/forge";

import {
	BrowserOpener,
	CliPlatform,
	ForgeAutostart,
	make_browser_opener_layer,
	make_cli_platform_layer,
	make_windows_autostart_layer,
} from "./adapters";
import { DistributionOperations, DistributionOperationsError } from "./distribution";
import { ForgeLifecycle, make_forge_lifecycle_layer } from "./lifecycle";
import { ForgeControlLive } from "./node-control";
import { make_node_forge_launcher_layer } from "./node-launcher";
import { make_node_profile_store_layer } from "./node-profile-store";
import { make_node_distribution_runtime_layer } from "./node-distribution-runtime";
import { ForgeProfileStore } from "./profile";
import { ForgeOperations, make_forge_operations_layer } from "./operations";
import { make_forge_protocol_handler_layer } from "./protocol-handler";

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
const fix = Flag.boolean("fix").pipe(Flag.withDescription("Repair safe local Forge prerequisites"));
const foreground = Flag.boolean("foreground").pipe(
	Flag.withDescription("Run attached to this terminal"),
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
const remove_data = Flag.boolean("remove-data").pipe(
	Flag.withDescription("Also permanently remove Forge profiles and conversation data"),
);

/** Stops every profile inventoried by Forge before product files are released. */
export const StopAllForgeProfiles = Effect.gen(function* () {
	const profile_store = yield* ForgeProfileStore;
	const lifecycle = yield* ForgeLifecycle;
	yield* Effect.forEach(
		yield* profile_store.List(),
		(profile_name) => lifecycle.Stop(profile_name),
		{ concurrency: 1, discard: true },
	);
});

export const AeCommand = Command.make("ae", {}, () =>
	Effect.gen(function* () {
		const report = yield* (yield* DistributionOperations).Doctor();
		if (!report.healthy) {
			return yield* new DistributionOperationsError({
				code: "installation_partial",
				operation: "doctor",
			});
		}
		const url = yield* (yield* ForgeLifecycle).Open("default");
		yield* (yield* BrowserOpener).Open(url);
	}),
).pipe(
	Command.withSubcommands([
		Command.make("setup", { autostart, data_root, listen_port, mode, profile }, (input) =>
			Effect.gen(function* () {
				const store = yield* ForgeProfileStore;
				const platform = yield* CliPlatform;
				yield* store.Ensure(input.profile, {
					data_root: Option.getOrElse(
						input.data_root,
						() => `${platform.home}/profiles/${input.profile}/data`,
					),
					listen_host: "127.0.0.1",
					listen_port: Option.getOrElse(input.listen_port, () => 0),
					mode: input.mode,
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
		Command.make("instances", { json }, (input) =>
			Effect.gen(function* () {
				const registry_root = ResolveInstanceRegistryRoot(process.env);
				const instances =
					registry_root === undefined ? [] : yield* ListForgeInstances(registry_root);
				if (input.json) {
					yield* Console.log(JSON.stringify({ instances }));
					return;
				}
				if (instances.length === 0) {
					yield* Console.log("No running Forge instances found");
					return;
				}
				for (const instance of instances) {
					yield* Console.log(
						`${instance.profile}\t${instance.endpoint}\tpid ${instance.pid}`,
					);
				}
			}),
		).pipe(Command.withDescription("List every running Forge announced on this machine")),
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
		Command.make("doctor", { fix, json, profile }, (input) =>
			Effect.gen(function* () {
				const operations = yield* ForgeOperations;
				const forge = input.fix
					? yield* operations.Repair(input.profile)
					: yield* operations.Doctor(input.profile);
				const distribution = input.fix
					? yield* (yield* DistributionOperations).Repair()
					: yield* (yield* DistributionOperations).Doctor();
				const result = { distribution, forge };
				yield* Console.log(
					input.json
						? JSON.stringify(result)
						: [
								...forge.checks.map(
									(check) => `${check.state}: ${check.name} — ${check.detail}`,
								),
								`${distribution.healthy ? "ok" : "error"}: installation — ${distribution.installation}`,
								...distribution.integrations.map(
									(integration) =>
										`${integration._tag === "Owned" ? "ok" : "error"}: ${integration.kind} — ${integration._tag}`,
								),
							].join("\n"),
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
		Command.make("update", {}, () =>
			Effect.gen(function* () {
				const outcome = yield* (yield* DistributionOperations).Update();
				yield* Console.log(
					outcome._tag === "Current"
						? `Artisan ${outcome.manifest.active_version} is current`
						: `Artisan ${outcome.manifest.active_version} is ready`,
				);
			}),
		).pipe(Command.withDescription("Update Artisan through the signed release channel")),
		Command.make("uninstall", { remove_data }, (input) =>
			Effect.gen(function* () {
				yield* StopAllForgeProfiles;
				const result = yield* (yield* DistributionOperations).Uninstall(input.remove_data);
				yield* Console.log(
					result.forge_data_removed
						? "Scheduled Artisan removal and removed Forge data"
						: "Scheduled Artisan removal; Forge data was retained",
				);
				yield* Console.log(
					`If detached cleanup fails, run: ${result.manual_cleanup_command}`,
				);
			}),
		).pipe(
			Command.withDescription(
				"Uninstall Artisan while retaining Forge data unless --remove-data is explicit",
			),
		),
	]),
);

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
const ForgeProtocolHandlerLive = make_forge_protocol_handler_layer.pipe(
	Layer.provide(NodeFileSystem.layer),
);
const ForgeOperationsLive = make_forge_operations_layer.pipe(
	Layer.provide(
		Layer.mergeAll(
			ProfileStoreLive,
			ForgeLifecycleLive,
			make_windows_autostart_layer(),
			ForgeProtocolHandlerLive,
		),
	),
);

const AeRuntimeLive = Layer.mergeAll(
	NodePlatformLive,
	ProfileStoreLive,
	ForgeLauncherLive,
	ForgeControlLive,
	ForgeLifecycleLive,
	ForgeOperationsLive,
	ForgeProtocolHandlerLive,
	make_node_distribution_runtime_layer(),
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
