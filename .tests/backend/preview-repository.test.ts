import { Effect, Option, Schema } from "effect";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { Layer, ManagedRuntime } from "effect";
import { describe, expect, it } from "vitest";

import { EventEnvelope, EventPayload } from "@artisan/protocol";

import {
	ValidateLocalPreviewUrl,
	ValidatePreviewRegistrationPort,
} from "../../modules/backend/src/preview/preview-repository";
import {
	PreviewRepository,
	PreviewRepositoryLive,
} from "../../modules/backend/src/preview/preview-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import {
	JournalEvents,
	PreviewCommands,
	PreviewDispatchLeases,
	PreviewInspectionSessions,
	PreviewTargets,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const make_runtime = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-preview-repository-"));
	directories.push(directory);
	let id = 0;
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path: join(directory, "preview.db"), migrations_path }),
		JournalNotifierLive,
		Layer.succeed(RuntimeMetadata, {
			instance_id: "preview_test",
			MakeId: (prefix) => Effect.sync(() => `${prefix}_${++id}`),
			Now: Effect.succeed("2026-07-18T20:00:00.000Z"),
		}),
	);
	return ManagedRuntime.make(
		Layer.merge(PreviewRepositoryLive, JournalStoreLive).pipe(
			Layer.provideMerge(infrastructure),
		),
	);
};

describe("PreviewRepository storage boundary", () => {
	it("canonicalizes explicit loopback URLs and rejects public or credential-bearing URLs", async () => {
		const accepted = await Effect.runPromise(
			ValidateLocalPreviewUrl("http://localhost:5173/app"),
		);
		const public_error = await Effect.runPromise(
			ValidateLocalPreviewUrl("https://example.com/").pipe(Effect.flip),
		);
		const credential_error = await Effect.runPromise(
			ValidateLocalPreviewUrl("http://user:secret@127.0.0.1:3000/").pipe(Effect.flip),
		);

		expect(accepted).toBe("http://localhost:5173/app");
		expect(public_error.code).toBe("invalid");
		expect(credential_error.code).toBe("invalid");
	});

	it("requires the declared port to match explicit and implicit canonical URL ports", async () => {
		await expect(
			Effect.runPromise(ValidatePreviewRegistrationPort("http://localhost/", 80)),
		).resolves.toBe("http://localhost/");
		await expect(
			Effect.runPromise(ValidatePreviewRegistrationPort("https://[::1]/", 443)),
		).resolves.toBe("https://[::1]/");
		const mismatch = await Effect.runPromise(
			ValidatePreviewRegistrationPort("http://localhost:5173/", 4173).pipe(Effect.flip),
		);

		expect(mismatch).toMatchObject({ code: "invalid" });
	});

	it("persists every target and inspection lifecycle command atomically and abandons inspection after restart", async () => {
		const runtime = await make_runtime();
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;
					const repository = yield* PreviewRepository;
					yield* database.client.insert(Threads).values({
						activity_version: 0,
						created_at: "2026-07-18T20:00:00.000Z",
						current_goal: "preview",
						last_activity_at: "2026-07-18T20:00:00.000Z",
						live_status: "Idle",
						metadata_version: 0,
						pinned: false,
						thread_id: "thread_preview",
						title: "preview",
						title_locked: false,
						title_source: "initial",
						updated_at: "2026-07-18T20:00:00.000Z",
					});
					yield* repository.Register({
						message_id: "register",
						port: 5173,
						project_id: "project",
						routes: ["/", "/health"],
						source: { kind: "terminal", terminal_id: "terminal_preview" },
						target_id: "target",
						thread_id: "thread_preview",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					});
					for (const input of [
						{
							action: "probe",
							message_id: "probe",
							health_json:
								'{"checked_at":"2026-07-18T20:00:00.000Z","latency_ms":1,"status":"healthy","status_code":200}',
						},
						{ action: "state", message_id: "state", state: "healthy" as const },
						{
							action: "launch",
							message_id: "launch",
							launch_state: "launched" as const,
						},
					] as const)
						yield* repository.UpdateTarget({
							...input,
							target_id: "target",
							thread_id: "thread_preview",
						});
					yield* repository.UpdateInspection({
						action: "inspection_open",
						connector_id: "connector",
						message_id: "open",
						session_id: "session",
						target_id: "target",
						thread_id: "thread_preview",
					});
					yield* repository.UpdateInspection({
						action: "inspection_reconnect",
						message_id: "reconnect",
						reconnect_state: "reconnecting",
						session_id: "session",
						thread_id: "thread_preview",
					});
					const recovered = yield* repository.RecoverInspections();
					yield* repository.UpdateTarget({
						action: "remove",
						message_id: "remove",
						target_id: "target",
						thread_id: "thread_preview",
					});
					return {
						commands: yield* database.client.select().from(PreviewCommands),
						events: yield* database.client.select().from(JournalEvents),
						replay: yield* journal.ReadReplay({ after_journal_sequence: 0 }),
						inspections: yield* database.client
							.select()
							.from(PreviewInspectionSessions),
						recovered,
						targets: yield* database.client.select().from(PreviewTargets),
					};
				}),
			);
			expect(result.commands.map((command) => command.action)).toEqual([
				"register",
				"probe",
				"state",
				"launch",
				"inspection_open",
				"inspection_reconnect",
				"remove",
			]);
			expect(result.events.map((event) => event.stream_sequence)).toEqual([
				1, 2, 3, 4, 5, 6, 7, 8,
			]);
			expect(result.inspections[0]).toMatchObject({
				state: "abandoned",
				reconnect_state: "unavailable",
				last_error: "backend_restart",
			});
			expect(result.recovered).toHaveLength(1);
			expect(result.targets[0]).toMatchObject({
				state: "removed",
				launch_state: "launched",
				routes_json: '["/","/health"]',
				source_kind: "terminal",
				source_id: "terminal_preview",
			});
			for (const event of result.events)
				expect(() =>
					Schema.decodeUnknownSync(EventPayload)(JSON.parse(event.payload_json)),
				).not.toThrow();
			for (const event of result.replay)
				expect(() => Schema.decodeUnknownSync(EventEnvelope)(event)).not.toThrow();
		} finally {
			await runtime.dispose();
			await Promise.all(
				directories
					.splice(0)
					.map((directory) => rm(directory, { force: true, recursive: true })),
			);
		}
	});

	it("replays exact command IDs, rejects conflicting intent, and fences erasing threads", async () => {
		const runtime = await make_runtime();
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* PreviewRepository;
					yield* database.client.insert(Threads).values({
						activity_version: 0,
						created_at: "2026-07-18T20:00:00.000Z",
						current_goal: "preview",
						last_activity_at: "2026-07-18T20:00:00.000Z",
						live_status: "Idle",
						metadata_version: 0,
						pinned: false,
						thread_id: "thread_fenced",
						title: "preview",
						title_locked: false,
						title_source: "initial",
						updated_at: "2026-07-18T20:00:00.000Z",
					});
					const register = {
						message_id: "exact_register",
						port: 5173,
						project_id: "project",
						target_id: "target_exact",
						thread_id: "thread_fenced",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					} as const;
					yield* repository.Register(register);
					const duplicate = yield* repository.Register(register);
					const conflict = yield* repository
						.Register({ ...register, url: "http://localhost:4173/" })
						.pipe(Effect.exit);
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-18T20:00:01.000Z",
						thread_id: "thread_fenced",
					});
					const fenced = yield* repository
						.UpdateTarget({
							action: "state",
							message_id: "fenced",
							state: "healthy",
							target_id: "target_exact",
							thread_id: "thread_fenced",
						})
						.pipe(Effect.exit);
					return {
						commands: yield* database.client.select().from(PreviewCommands),
						conflict,
						duplicate,
						fenced,
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);
			expect(result.duplicate.target_id).toBe("target_exact");
			expect(result.conflict._tag).toBe("Failure");
			expect(result.fenced._tag).toBe("Failure");
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
			await Promise.all(
				directories
					.splice(0)
					.map((directory) => rm(directory, { force: true, recursive: true })),
			);
		}
	});

	it("allows one launch claim and makes exact retries side-effect free", async () => {
		const runtime = await make_runtime();
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* PreviewRepository;
					yield* database.client.insert(Threads).values({
						activity_version: 0,
						created_at: "2026-07-18T20:00:00.000Z",
						current_goal: "preview",
						last_activity_at: "2026-07-18T20:00:00.000Z",
						live_status: "Idle",
						metadata_version: 0,
						pinned: false,
						thread_id: "thread_launch",
						title: "preview",
						title_locked: false,
						title_source: "initial",
						updated_at: "2026-07-18T20:00:00.000Z",
					});
					yield* repository.Register({
						message_id: "launch_register",
						port: 5173,
						project_id: "project",
						target_id: "target_launch",
						thread_id: "thread_launch",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					});
					const claim = {
						action: "launch" as const,
						launch_state: "launching" as const,
						message_id: "launch_one",
						target_id: "target_launch",
						thread_id: "thread_launch",
					};
					yield* repository.UpdateTarget(claim);
					const retry = yield* repository.UpdateTarget(claim);
					const competing = yield* repository
						.UpdateTarget({ ...claim, message_id: "launch_two" })
						.pipe(Effect.exit);
					const completed = {
						...claim,
						launch_state: "launched" as const,
						message_id: "launch_result",
					};
					yield* repository.UpdateTarget(completed);
					const replay = yield* repository.ReplayTargetUpdate(completed);
					return {
						commands: yield* database.client.select().from(PreviewCommands),
						competing,
						events: yield* database.client.select().from(JournalEvents),
						retry,
						replay,
					};
				}),
			);
			expect(result.retry.launch_state).toBe("launching");
			expect(Option.getOrThrow(result.replay).launch_state).toBe("launched");
			expect(result.competing._tag).toBe("Failure");
			expect(result.commands).toHaveLength(3);
			expect(result.events).toHaveLength(3);
		} finally {
			await runtime.dispose();
			await Promise.all(
				directories
					.splice(0)
					.map((directory) => rm(directory, { force: true, recursive: true })),
			);
		}
	});

	it("keeps a durable dispatch lease across an erasure claim and expires it without replay", async () => {
		const runtime = await make_runtime();
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* PreviewRepository;
					yield* database.client.insert(Threads).values({
						activity_version: 0,
						created_at: "2026-07-18T20:00:00.000Z",
						current_goal: "preview",
						last_activity_at: "2026-07-18T20:00:00.000Z",
						live_status: "Idle",
						metadata_version: 0,
						pinned: false,
						thread_id: "thread_lease",
						title: "preview",
						title_locked: false,
						title_source: "initial",
						updated_at: "2026-07-18T20:00:00.000Z",
					});
					yield* repository.Register({
						message_id: "lease_register",
						port: 5173,
						project_id: "project",
						target_id: "target_lease",
						thread_id: "thread_lease",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					});
					const lease = yield* repository.AcquireDispatchLease({
						kind: "launch",
						target_id: "target_lease",
						thread_id: "thread_lease",
					});
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-18T20:00:01.000Z",
						thread_id: "thread_lease",
					});
					const while_owned = yield* repository.UpdateTarget(
						{
							action: "launch",
							launch_state: "launching",
							message_id: "lease_intent",
							target_id: "target_lease",
							thread_id: "thread_lease",
						},
						lease.lease_id,
					);
					yield* repository.ReleaseDispatchLease(lease);
					const after_release = yield* repository
						.AcquireDispatchLease({
							kind: "probe",
							target_id: "target_lease",
							thread_id: "thread_lease",
						})
						.pipe(Effect.exit);
					yield* database.client.delete(ThreadErasureClaims);
					yield* database.client.insert(PreviewDispatchLeases).values({
						acquired_at: "2026-07-18T19:00:00.000Z",
						expires_at: "2026-07-18T19:01:00.000Z",
						kind: "launch",
						lease_id: "expired_lease",
						owner_instance_id: "crashed_runtime",
						session_id: null,
						target_id: "target_lease",
						thread_id: "thread_lease",
					});
					const recovered = yield* repository.RecoverDispatchLeases();
					return { after_release, recovered, while_owned };
				}),
			);
			expect(result.while_owned.launch_state).toBe("launching");
			expect(result.after_release._tag).toBe("Failure");
			expect(result.recovered.map((lease) => lease.lease_id)).toEqual(["expired_lease"]);
		} finally {
			await runtime.dispose();
			await Promise.all(
				directories
					.splice(0)
					.map((directory) => rm(directory, { force: true, recursive: true })),
			);
		}
	});
});
