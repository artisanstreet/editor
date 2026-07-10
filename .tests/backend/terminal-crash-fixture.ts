import { Effect, Layer, Stream } from "effect";

import { make_backend_runtime, ProtocolRouter, TerminalDriver } from "@artisan/backend";
import type { CommandEnvelope } from "@artisan/protocol";

const database_path = process.env.ARTISAN_TERMINAL_CRASH_DATABASE;
const migrations_path = process.env.ARTISAN_TERMINAL_CRASH_MIGRATIONS;

if (!database_path || !migrations_path) {
	throw new Error("Terminal crash fixture paths are required");
}

const TerminalDriverTest = Layer.succeed(TerminalDriver, {
	Open: () =>
		Effect.gen(function* () {
			yield* Effect.addFinalizer(() => Effect.void);

			return {
				Clear: Effect.void,
				Close: Effect.never,
				Exit: Effect.never,
				Kill: () => Effect.never,
				Output: Stream.empty,
				Resize: () => Effect.void,
				Write: () => Effect.void,
				pid: 42_424,
			};
		}),
});
const runtime = make_backend_runtime({
	database_path,
	migrations_path,
	terminal_driver: TerminalDriverTest,
});

const Route = (command: CommandEnvelope) =>
	runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(command);
		}),
	);

await Route({
	kind: "command",
	message_id: "crash_thread_create",
	origin: "frontend",
	payload: { title: "Crash recovery", type: "thread.create" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-10T08:00:00.000Z",
	thread_id: "thread_crash",
});
await Route({
	kind: "command",
	message_id: "crash_terminal_open",
	origin: "frontend",
	payload: {
		args: [],
		cols: 80,
		executable: "fake-crash-shell",
		rows: 24,
		terminal_id: "terminal_crash",
		type: "terminal.open",
		working_directory: process.cwd(),
		workspace_id: "workspace_crash",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-07-10T08:00:01.000Z",
	thread_id: "thread_crash",
});

process.stdout.write("TERMINAL_CRASH_READY\n");

await new Promise<never>(() => {});
