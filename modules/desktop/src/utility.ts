import { fileURLToPath } from "node:url";

import { Effect, Layer, ManagedRuntime } from "effect";

import { make_desktop_backend_layer, RichLinkAssetStoreLive } from "@artisan/backend";
import {
	CodexEngine,
	CodexProcessFactoryLive,
	make_codex_engine_layer,
} from "@artisan/engines";
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

function parent_message(value: unknown): { readonly generation: number; readonly kind: string } | undefined {
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

/** Starts the desktop-only backend runtime and serves each current connection generation. */
export const StartUtility = async () => {
	const environment = read_environment();
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

				const raw_ports = event.ports as ReadonlyArray<{ readonly close: () => void }>;

				active_generations.set(generation, raw_ports);
				let serve: Promise<void>;
				serve = transport_runtime
					.runPromise(
						Effect.scoped(
							Effect.gen(function* () {
								const control_port = yield* adapt_electron_message_port_main(raw_ports[0] as never);
								const stream_port = yield* adapt_electron_message_port_main(raw_ports[1] as never);

								yield* server.Serve({ control_port, stream_port });
							}),
						),
					)
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
	void StartUtility();
}
