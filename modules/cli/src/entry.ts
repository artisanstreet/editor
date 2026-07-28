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
import { make_node_instance_store_layer } from "./node-instance-store";
import { make_node_distribution_runtime_layer } from "./node-distribution-runtime";
import { ForgeInstanceStore } from "./instance";
import { ForgeOperations, make_forge_operations_layer } from "./operations";
import { make_forge_protocol_handler_layer } from "./protocol-handler";

/**
 * Declarative command surface. Runtime composition is deliberately exported by
 * the module, so packaging can provide Forge adapters without this entrypoint
 * importing a concurrently renamed host package.
 */
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
const serve_frontend = Flag.boolean("serve-frontend").pipe(
	Flag.withDescription("Serve the bundled web frontend (development homes only)"),
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
	Flag.withDescription("Also permanently remove Forge data and conversations"),
);
const handoff = Flag.boolean("handoff").pipe(
	Flag.withDescription(
		"Print a one-time {endpoint, pair_code} JSON handoff for a trusted local caller",
	),
);

/** Stops this home's Forge instance before product files are released. */
export const StopForgeInstance = Effect.gen(function* () {
	const lifecycle = yield* ForgeLifecycle;
	yield* lifecycle.Stop();
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
		const url = yield* (yield* ForgeLifecycle).Open();
		yield* (yield* BrowserOpener).Open(url);
	}),
).pipe(
	Command.withSubcommands([
		Command.make(
			"setup",
			{ autostart, data_root, listen_port, mode, serve_frontend },
			(input) =>
				Effect.gen(function* () {
					const store = yield* ForgeInstanceStore;
					const platform = yield* CliPlatform;
					yield* store.Ensure({
						data_root: Option.getOrElse(input.data_root, () => `${platform.home}/data`),
						listen_host: "127.0.0.1",
						listen_port: Option.getOrElse(input.listen_port, () => 0),
						mode: input.mode,
						serve_frontend: input.serve_frontend,
						version: 1,
					});
					if (input.autostart)
						yield* (yield* ForgeAutostart).Configure({ enabled: true });
					yield* Console.log("Configured Forge");
				}),
		).pipe(Command.withDescription("Create or update this home's loopback-only Forge")),
		Command.make("start", { foreground }, (input) =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Start(input.foreground);
			}),
		).pipe(Command.withDescription("Start Forge in the background")),
		Command.make("stop", {}, () =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Stop();
			}),
		).pipe(Command.withDescription("Stop the Forge instance")),
		Command.make("restart", { foreground }, (input) =>
			Effect.gen(function* () {
				yield* (yield* ForgeLifecycle).Restart(input.foreground);
			}),
		).pipe(Command.withDescription("Restart the Forge instance")),
		Command.make("status", { json }, (input) =>
			Effect.gen(function* () {
				const result = yield* (yield* ForgeLifecycle).Status();
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
					yield* Console.log(`${instance.endpoint}\tpid ${instance.pid}`);
				}
			}),
		).pipe(Command.withDescription("List every running Forge announced on this machine")),
		Command.make("logs", { follow, lines }, (input) =>
			Effect.gen(function* () {
				const operations = yield* ForgeOperations;
				const recent = yield* operations.ReadLogs(input.lines);
				yield* Effect.forEach(recent, (line) => Console.log(line));
				if (input.follow)
					yield* operations
						.FollowLogs()
						.pipe(Stream.runForEach((chunk) => Console.log(chunk)));
			}),
		).pipe(Command.withDescription("Show or follow Forge log output")),
		Command.make("doctor", { fix, json }, (input) =>
			Effect.gen(function* () {
				const operations = yield* ForgeOperations;
				const forge = input.fix ? yield* operations.Repair() : yield* operations.Doctor();
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
		Command.make("open", { handoff, origin }, (input) =>
			Effect.gen(function* () {
				const lifecycle = yield* ForgeLifecycle;
				if (input.handoff) {
					/**
					 * The capability is one-time and short-lived; stdout reaches
					 * only the trusted local process that invoked this mode, so no
					 * secret ever travels through argv or a browser navigation.
					 */
					const exchange = yield* lifecycle.PairHandoff();
					yield* Console.log(JSON.stringify({ ...exchange, version: 1 }));
					return;
				}
				const url = yield* lifecycle.Open(Option.getOrUndefined(input.origin));
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
				yield* StopForgeInstance;
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

const InstanceStoreLive = make_node_instance_store_layer();
const ForgeLauncherLive = make_node_forge_launcher_layer.pipe(Layer.provide(InstanceStoreLive));
const ForgeLifecycleLive = make_forge_lifecycle_layer.pipe(
	Layer.provide(Layer.mergeAll(InstanceStoreLive, ForgeLauncherLive, ForgeControlLive)),
);
const ForgeProtocolHandlerLive = make_forge_protocol_handler_layer.pipe(
	Layer.provide(NodeFileSystem.layer),
);
const ForgeOperationsLive = make_forge_operations_layer.pipe(
	Layer.provide(
		Layer.mergeAll(
			InstanceStoreLive,
			ForgeLifecycleLive,
			make_windows_autostart_layer(),
			ForgeProtocolHandlerLive,
		),
	),
);

const AeRuntimeLive = Layer.mergeAll(
	NodePlatformLive,
	InstanceStoreLive,
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
