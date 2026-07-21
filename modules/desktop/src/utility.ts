import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";

import { make_desktop_backend_layer, RichLinkAssetStoreLive } from "@artisan/backend";
import { CodexEngine, CodexProcessFactoryLive, make_codex_engine_layer } from "@artisan/engines";
import {
	make_backend_message_port_transport_server_layer,
	MessagePortTransportServer,
} from "@artisan/transport/server";
import { adapt_electron_message_port_main } from "@artisan/transport/electron-shapes";

interface ParentPortMessage {
	readonly data: unknown;
	readonly ports: ReadonlyArray<unknown>;
}

interface UtilityEnvironment {
	readonly database_path: string;
	readonly migrations_path: string;
}

const report_utility_diagnostic = (kind: string, value: Readonly<Record<string, unknown>> = {}) =>
	process.parentPort?.postMessage({ kind, ...value });

const describe_error = (cause: unknown, depth = 0): unknown => {
	if (depth >= 4 || typeof cause !== "object" || cause === null) return String(cause);
	const candidate = cause as Record<string, unknown>;
	return {
		_tag: candidate._tag,
		cause: describe_error(candidate.cause, depth + 1),
		code: candidate.code,
		dropped_messages: candidate.dropped_messages,
		message: cause instanceof Error ? cause.message : candidate.message,
		name: cause instanceof Error ? cause.name : candidate.name,
	};
};

const error_diagnostic = (cause: unknown) => ({
	details: describe_error(cause),
	message: cause instanceof Error ? cause.message : "Unknown utility failure",
	stack: cause instanceof Error ? cause.stack : undefined,
});

function parent_message(
	value: unknown,
): { readonly generation: number; readonly kind: string } | undefined {
	if (typeof value !== "object" || value === null) {
		return undefined;
	}

	const candidate = value as { readonly generation?: unknown; readonly kind?: unknown };

	return typeof candidate.kind === "string" && typeof candidate.generation === "number"
		? { generation: candidate.generation, kind: candidate.kind }
		: undefined;
}

function read_environment(): UtilityEnvironment {
	const database_path = process.env.ARTISAN_DATABASE_PATH;
	const migrations_path = process.env.ARTISAN_MIGRATIONS_PATH;

	if (!database_path || !migrations_path) {
		throw new Error("Artisan utility process requires explicit database and migration paths");
	}

	return { database_path, migrations_path };
}

/** Opens the exact staged native modules, without consulting externally injected NODE_PATH entries. */
function verify_packaged_native_runtime() {
	const require = createRequire(import.meta.url);
	const koffi_native_binding_path =
		require.resolve("./native-runtime/@koromix/koffi-win32-x64/win32_x64/koffi.node");
	const koffi_module_path = require.resolve("./native-runtime/@koromix/koffi-win32-x64");
	const koffi = require(koffi_module_path) as { readonly version?: string };
	if (koffi.version !== "3.1.1") {
		throw new Error("Packaged Koffi runtime did not load the expected native binding");
	}
	const node_pty_module_path = require.resolve("./native-runtime/node-pty");
	const node_pty = require(node_pty_module_path) as {
		readonly spawn: (
			file: string,
			args: ReadonlyArray<string>,
			options: Readonly<Record<string, unknown>>,
		) => { readonly kill: () => void };
	};
	const terminal = node_pty.spawn(process.execPath, ["-e", "process.exit(0)"], {
		cols: 1,
		cwd: tmpdir(),
		env: { ...process.env },
		name: "xterm-256color",
		rows: 1,
	});
	terminal.kill();

	const bounded_native_binding_path =
		require.resolve("./native-runtime/@artisan/bounded-file-store-native/bounded_file_store_native.win32-x64-msvc.node");
	const bounded_native = require(bounded_native_binding_path) as {
		readonly NativeBoundedRegularFileStore: new (
			root: string,
			receipt_authentication_key: Uint8Array,
		) => { readonly close: () => unknown };
	};
	const smoke_root = process.env.ARTISAN_PACKAGED_SMOKE_ROOT;
	if (!smoke_root) {
		throw new Error("Packaged desktop smoke requires an isolated smoke root");
	}
	const store_root = join(smoke_root, "native-store");
	mkdirSync(store_root, { recursive: true });
	const store = new bounded_native.NativeBoundedRegularFileStore(store_root, new Uint8Array(32));
	store.close();

	return {
		bounded_native_binding_path,
		koffi_native_binding_path,
		native_store_root: store_root,
		node_pty_module_path,
	};
}

/** Starts the desktop-only backend runtime and serves each current connection generation. */
export const StartUtility = async () => {
	const environment = read_environment();
	mkdirSync(dirname(environment.database_path), { recursive: true });
	if (process.env.ARTISAN_PACKAGED_SMOKE === "1") {
		const native_load = verify_packaged_native_runtime();
		process.parentPort?.postMessage({ kind: "artisan:smoke-native-load", ...native_load });
	}
	const engine_runtime = ManagedRuntime.make(
		make_codex_engine_layer().pipe(Layer.provide(CodexProcessFactoryLive)),
	);
	const codex_engine = await engine_runtime.runPromise(CodexEngine);
	const backend_layer = make_desktop_backend_layer({
		database_path: environment.database_path,
		engines: [codex_engine],
		migrations_path: environment.migrations_path,
	});
	const transport_runtime = ManagedRuntime.make(
		make_backend_message_port_transport_server_layer().pipe(
			Layer.provideMerge(backend_layer),
			Layer.provideMerge(RichLinkAssetStoreLive),
		),
	);
	const server = await transport_runtime.runPromise(MessagePortTransportServer);
	report_utility_diagnostic("artisan:utility-ready");
	const active_generations = new Map<number, ReadonlyArray<{ readonly close: () => void }>>();
	const active_serves = new Set<Promise<void>>();
	let shutting_down = false;
	let shutdown_promise: Promise<void> | undefined;

	const ShutdownUtility = () => {
		if (shutdown_promise) {
			return shutdown_promise;
		}

		shutting_down = true;
		shutdown_promise = (async () => {
			for (const ports of active_generations.values()) {
				for (const port of ports) {
					port.close();
				}
			}
			active_generations.clear();
			await Promise.allSettled(active_serves);
			await transport_runtime.dispose();
			await engine_runtime.dispose();
			process.parentPort?.postMessage({ kind: "artisan:shutdown-complete" });
		})();

		return shutdown_promise;
	};

	process.parentPort?.on("message", (event: ParentPortMessage) => {
		void Effect.runPromise(
			Effect.gen(function* () {
				if (
					typeof event.data === "object" &&
					event.data !== null &&
					(event.data as { readonly kind?: unknown }).kind === "artisan:shutdown"
				) {
					yield* Effect.promise(ShutdownUtility);
					return;
				}

				if (shutting_down) {
					return;
				}

				const message = parent_message(event.data);

				if (message === undefined || !Number.isSafeInteger(message.generation)) {
					return;
				}

				const generation = message.generation;

				if (message.kind === "artisan:close-generation") {
					for (const port of active_generations.get(generation) ?? []) {
						port.close();
					}
					active_generations.delete(generation);
					return;
				}

				if (message.kind !== "artisan:connect" || event.ports.length !== 2) {
					return;
				}
				report_utility_diagnostic("artisan:utility-connect", {
					generation,
					port_count: event.ports.length,
				});

				const raw_ports = event.ports as ReadonlyArray<{ readonly close: () => void }>;

				active_generations.set(generation, raw_ports);
				let serve: Promise<void>;
				serve = transport_runtime
					.runPromise(
						Effect.scoped(
							Effect.gen(function* () {
								const control_port = yield* adapt_electron_message_port_main(
									raw_ports[0] as never,
								);
								const stream_port = yield* adapt_electron_message_port_main(
									raw_ports[1] as never,
								);

								yield* server.Serve({ control_port, stream_port });
							}),
						),
					)
					.catch((cause) => {
						report_utility_diagnostic("artisan:utility-serve-failure", {
							generation,
							...error_diagnostic(cause),
						});
					})
					.finally(() => {
						active_generations.delete(generation);
						active_serves.delete(serve);
					});
				active_serves.add(serve);
			}),
		).catch(() => undefined);
	});
};

if (process.argv[1] === fileURLToPath(import.meta.url)) {
	void StartUtility().catch((cause) => {
		report_utility_diagnostic("artisan:utility-fatal", error_diagnostic(cause));
	});
}
