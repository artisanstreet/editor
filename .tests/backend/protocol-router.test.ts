import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";
import { ProtocolRouter, make_backend_runtime } from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_command(
	message_id = "message_1",
	thread_id = "thread_1",
	title = "Backend foundation",
): CommandEnvelope {
	return {
		protocol_version: 1,
		schema_version: 1,
		kind: "command",
		message_id,
		thread_id,
		origin: "frontend",
		sent_at: "2026-07-10T08:00:00.000Z",
		payload: {
			type: "thread.create",
			title,
		},
	};
}

async function route(runtime: ReturnType<typeof make_backend_runtime>, input: unknown) {
	return runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			return yield* router.Route(input);
		}),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((directory) =>
			rm(directory, {
				force: true,
				recursive: true,
			}),
		),
	);
});

describe("protocol router", () => {
	it("durably accepts a command and emits its correlated event", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const output = await route(runtime, make_command());
			const [receipt, event] = output;

			expect(receipt).toMatchObject({
				kind: "command.receipt",
				correlation_id: "message_1",
				thread_id: "thread_1",
				payload: {
					journal_sequence: 1,
					status: "accepted",
				},
			});

			expect(event).toMatchObject({
				kind: "event",
				correlation_id: "message_1",
				thread_id: "thread_1",
				stream_id: "thread:thread_1",
				sequence: 1,
				journal_sequence: 1,
				payload: {
					type: "thread.created",
					title: "Backend foundation",
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("deduplicates a retried command after the backend restarts", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
		});

		let first_output: Awaited<ReturnType<typeof route>>;

		try {
			first_output = await route(first_runtime, make_command());
		} finally {
			await first_runtime.dispose();
		}

		const first_event = first_output[1];
		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
		});

		try {
			const second_output = await route(second_runtime, make_command());
			const [receipt, event] = second_output;

			expect(receipt).toMatchObject({
				kind: "command.receipt",
				payload: {
					journal_sequence: 1,
					status: "duplicate",
				},
			});

			expect(event).toEqual(first_event);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("returns a correlated rejection when a command id changes intent", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await route(runtime, make_command());

			const [receipt] = await route(
				runtime,
				make_command("message_1", "thread_1", "Different title"),
			);

			expect(receipt).toMatchObject({
				kind: "command.receipt",
				correlation_id: "message_1",
				payload: {
					status: "rejected",
					error: {
						code: "command.id_conflict",
						retryable: false,
					},
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects metadata changes on a retried command id", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const command = {
			...make_command(),
			raw_origin: {
				provider: "codex",
				reference: "native_1",
			},
		};

		try {
			await route(runtime, command);

			const [receipt] = await route(runtime, {
				...command,
				raw_origin: {
					provider: "codex",
					reference: "native_2",
				},
			});

			expect(receipt).toMatchObject({
				kind: "command.receipt",
				payload: {
					status: "rejected",
					error: {
						code: "command.id_conflict",
					},
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("does not persist a command that fails before acceptance", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			await route(runtime, make_command());

			const [rejected] = await route(runtime, make_command("message_2", "thread_1"));

			expect(rejected).toMatchObject({
				kind: "command.receipt",
				payload: {
					status: "rejected",
					error: {
						code: "thread.already_exists",
					},
				},
			});

			const [accepted] = await route(runtime, make_command("message_2", "thread_2"));

			expect(accepted).toMatchObject({
				kind: "command.receipt",
				payload: {
					status: "accepted",
					journal_sequence: 2,
				},
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("orders events independently within each thread stream", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const first_output = await route(runtime, make_command());
			const second_output = await route(runtime, make_command("message_2", "thread_2"));

			expect(first_output[1]).toMatchObject({
				stream_id: "thread:thread_1",
				sequence: 1,
				journal_sequence: 1,
			});

			expect(second_output[1]).toMatchObject({
				stream_id: "thread:thread_2",
				sequence: 1,
				journal_sequence: 2,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("turns malformed input into an uncorrelated protocol error", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const [error] = await route(runtime, {
				...make_command(),
				sent_at: "not-a-date",
			});

			expect(error).toMatchObject({
				kind: "protocol.error",
				payload: {
					code: "protocol.invalid_message",
					retryable: false,
				},
			});

			expect(error).not.toHaveProperty("correlation_id");
		} finally {
			await runtime.dispose();
		}
	});
});
