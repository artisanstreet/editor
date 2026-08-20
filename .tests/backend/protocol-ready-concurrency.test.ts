import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, Ref, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	HelloEnvelope,
	OutboundControlEnvelope,
	ProjectDirectoryPickEnvelope,
	ThreadListQueryEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	NativeDirectoryPicker,
	ProtocolServer,
	type ProtocolConnection,
} from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-editor-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

const MakeHello = (): HelloEnvelope => ({
	kind: "hello",
	message_id: "hello_concurrency",
	origin: "frontend",
	payload: {
		event_cursors: [],
		last_journal_sequence: 0,
		resume_mode: "fresh",
		supported_protocol_versions: [1],
	},
	schema_version: 1,
	sent_at: "2026-08-14T08:00:00.000Z",
});

const MakePick = (): ProjectDirectoryPickEnvelope => ({
	kind: "project.directory.pick",
	message_id: "pick_concurrency",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-14T08:00:00.000Z",
});

const MakeThreadList = (): ThreadListQueryEnvelope => ({
	kind: "thread.list.query",
	message_id: "thread_list_concurrency",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-14T08:00:00.000Z",
});

const OpenConnection = Effect.gen(function* () {
	const protocol_server = yield* ProtocolServer;

	return yield* protocol_server.Open;
});

const TakeUntil = (
	connection: ProtocolConnection,
	predicate: (envelope: OutboundControlEnvelope) => boolean,
) => connection.Outbound.pipe(Stream.takeUntil(predicate), Stream.runCollect);

const Negotiate = (connection: ProtocolConnection) =>
	Effect.gen(function* () {
		yield* connection.Receive(MakeHello());
		yield* TakeUntil(connection, (envelope) => envelope.kind === "replay.complete");
	});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("negotiated protocol ready dispatch", () => {
	it("does not head-of-line block independent queries behind a native picker and interrupts it on close", async () => {
		const database_path = await make_database_path();
		const first_picker_started = await Effect.runPromise(Deferred.make<void>());
		const first_picker_release = await Effect.runPromise(Deferred.make<void>());
		const second_picker_started = await Effect.runPromise(Deferred.make<void>());
		const second_picker_release = await Effect.runPromise(Deferred.make<void>());
		const picker_attempt = await Effect.runPromise(Ref.make(0));
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			native_directory_picker: Layer.succeed(NativeDirectoryPicker, {
				Pick: () =>
					Ref.getAndUpdate(picker_attempt, (attempt) => attempt + 1).pipe(
						Effect.flatMap((attempt) =>
							attempt === 0
								? Deferred.succeed(first_picker_started, undefined).pipe(
										Effect.andThen(Deferred.await(first_picker_release)),
									)
								: Deferred.succeed(second_picker_started, undefined).pipe(
										Effect.andThen(Deferred.await(second_picker_release)),
									),
						),
						Effect.as({ kind: "cancelled" as const }),
					),
			}),
		});

		try {
			const output = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenConnection;

						yield* Negotiate(connection);
						yield* connection.Receive(MakePick());
						yield* Deferred.await(first_picker_started);
						yield* connection.Receive(MakeThreadList());
						const response = yield* TakeUntil(
							connection,
							(envelope) =>
								"correlation_id" in envelope &&
								envelope.correlation_id === "thread_list_concurrency",
						);

						yield* Deferred.succeed(first_picker_release, undefined);
						const pick_response = yield* TakeUntil(
							connection,
							(envelope) =>
								"correlation_id" in envelope &&
								envelope.correlation_id === "pick_concurrency",
						);

						const closing_connection = yield* OpenConnection;

						yield* Negotiate(closing_connection);
						const closed_outbound = yield* closing_connection.Outbound.pipe(
							Stream.runCollect,
							Effect.forkChild,
						);
						yield* closing_connection.Receive(MakePick());

						yield* Deferred.await(second_picker_started);
						yield* closing_connection.Close;
						yield* Deferred.succeed(second_picker_release, undefined);
						const after_close = yield* Fiber.join(closed_outbound);

						return {
							after_close,
							pick_response,
							response,
						};
					}),
				),
			);

			expect(output.response).toMatchObject([
				{
					correlation_id: "thread_list_concurrency",
					kind: "thread.list.query.result",
					payload: { threads: [] },
				},
			]);
			expect(output.pick_response).toMatchObject([
				{
					correlation_id: "pick_concurrency",
					kind: "project.directory.pick.result",
					payload: { status: "cancelled" },
				},
			]);
			expect(output.after_close).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});
