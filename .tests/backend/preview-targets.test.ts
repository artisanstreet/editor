import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import {
	Deferred,
	Effect,
	Fiber,
	FileSystem,
	Layer,
	ManagedRuntime,
	Option,
	PubSub,
	Stream,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope as Command } from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalNotifier,
	JournalNotifierLive,
} from "../../modules/backend/src/persistence/journal-notifier";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	PreviewTargetProbeClaims,
	ThreadErasureClaims,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import {
	RuntimeMetadata,
	RuntimeMetadataLive,
} from "../../modules/backend/src/runtime/runtime-metadata";
import {
	PreviewHealthProbe,
	PreviewHealthProbeError,
	PreviewTarget,
	PreviewTargetClock,
	UnavailablePreviewHealthProbeLive,
} from "../../modules/backend/src/preview/preview-target";
import {
	PreviewTargetRepository,
	PreviewTargetRepositoryLive,
} from "../../modules/backend/src/preview/preview-target-repository";
import {
	make_preview_target_layer,
	type PreviewTargetOptions,
} from "../../modules/backend/src/preview/preview-target-service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const runtimes: Array<{ readonly dispose: () => Promise<void> }> = [];
const paths: Array<string> = [];
const now = "2026-07-14T22:00:00.000Z";

type RegisterPayload = Extract<Command["payload"], { readonly type: "preview.target.register" }>;
type RegisterCommand = Omit<Command, "payload"> & { readonly payload: RegisterPayload };
type ProbePayload = Extract<Command["payload"], { readonly type: "preview.target.probe" }>;
type ProbeCommand = Omit<Command, "payload"> & { readonly payload: ProbePayload };

interface TestRuntimeOptions {
	readonly metadata?: Layer.Layer<RuntimeMetadata>;
	readonly preview?: PreviewTargetOptions;
}

function register_command(
	message_id = "register_1",
	url = "http://localhost:5173/app",
): RegisterCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id: "preview_1",
			type: "preview.target.register",
			url,
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: now,
		thread_id: "thread_1",
	};
}

function probe_command(
	workspace_id = "workspace_1",
	message_id = "probe_1",
	target_id = "preview_1",
): ProbeCommand {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id,
			type: "preview.target.probe",
			workspace_id,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-14T22:00:01.000Z",
		thread_id: "thread_1",
	};
}

function remove_command(message_id = "remove_1"): Command {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: {
			project_id: "project_1",
			target_id: "preview_1",
			type: "preview.target.remove",
			workspace_id: "workspace_1",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-14T22:00:02.000Z",
		thread_id: "thread_1",
	};
}

const MakeDatabasePath = Effect.flatMap(FileSystem.FileSystem, (file_system) =>
	file_system.makeTempDirectory({ prefix: "artisan-preview-" }).pipe(
		Effect.tap((path) => Effect.sync(() => paths.push(path))),
		Effect.map((path) => `${path}/artisan.db`),
	),
);

function make_metadata_layer(instance_id: string, metadata_now: { value: string }) {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++identifier}`),
		Now: Effect.sync(() => metadata_now.value),
	});
}

function make_runtime(
	database_path: string,
	probe: Layer.Layer<PreviewHealthProbe> = UnavailablePreviewHealthProbeLive,
	options: TestRuntimeOptions = {},
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		JournalNotifierLive,
		options.metadata ?? RuntimeMetadataLive,
		Layer.succeed(PreviewTargetClock, { Now: Effect.succeed(10_000) }),
		probe,
	);
	const repository = PreviewTargetRepositoryLive.pipe(Layer.provide(infrastructure));
	const preview = make_preview_target_layer({
		sliding_event_capacity: 8,
		...options.preview,
	}).pipe(Layer.provide(repository), Layer.provide(infrastructure));
	const runtime = ManagedRuntime.make(Layer.mergeAll(preview, repository, infrastructure));

	runtimes.push(runtime);

	return runtime;
}

async function dispose_runtime(runtime: { readonly dispose: () => Promise<void> }) {
	const index = runtimes.indexOf(runtime);

	if (index >= 0) {
		runtimes.splice(index, 1);
	}

	await runtime.dispose();
}

const SeedThread = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: "thread_1",
		title: "Preview thread",
		title_source: "initial",
		updated_at: now,
	});
	yield* database.client.insert(EventStreams).values({
		last_sequence: 0,
		stream_id: "thread:thread_1",
	});
});

afterEach(async () => {
	await Promise.all(runtimes.splice(0).map((runtime) => runtime.dispose()));
	await Effect.runPromise(
		Effect.forEach(
			paths.splice(0),
			(path) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(path, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("PreviewTarget", () => {
	it("publishes one committed envelope and preserves strict local URL validation", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const database = yield* Database;
					const notifier = yield* JournalNotifier;
					const preview = yield* PreviewTarget;

					yield* SeedThread;

					const subscription = yield* notifier.Subscribe;
					const local_event_fiber = yield* preview.SlidingEvents.pipe(
						Stream.runHead,
						Effect.forkChild,
					);
					const notification_fiber = yield* PubSub.take(subscription).pipe(
						Effect.forkChild,
					);

					yield* Effect.yieldNow;

					const accepted = yield* preview.Register(register_command());
					const local_event = yield* Fiber.join(local_event_fiber);
					const notified_sequence = yield* Fiber.join(notification_fiber);
					const commands_after_notification = yield* database.client
						.select()
						.from(JournalCommands);
					const events_after_notification = yield* database.client
						.select()
						.from(JournalEvents);

					const duplicate_event_fiber = yield* preview.SlidingEvents.pipe(
						Stream.runHead,
						Effect.forkChild,
					);

					yield* Effect.yieldNow;

					const duplicate = yield* preview.Register(register_command());
					const duplicate_event = yield* Fiber.join(duplicate_event_fiber).pipe(
						Effect.timeoutOption("50 millis"),
					);
					const invalid = yield* preview
						.Register(register_command("invalid_1", "https://example.com/"))
						.pipe(Effect.flip);

					return {
						accepted,
						commands_after_notification,
						duplicate,
						duplicate_event,
						events_after_notification,
						invalid,
						local_event,
						notified_sequence,
					};
				}),
			),
		);

		expect(result.accepted.status).toBe("accepted");
		expect(result.duplicate.status).toBe("duplicate");
		expect(result.local_event).toEqual(Option.some(result.accepted.event));
		expect(result.notified_sequence).toBe(result.accepted.event.journal_sequence);
		expect(result.commands_after_notification).toHaveLength(1);
		expect(result.events_after_notification).toHaveLength(1);
		expect(Option.isNone(result.duplicate_event)).toBe(true);
		expect(result.invalid.code).toBe("invalid_target");
	});

	it("rejects exact replay after an erasure claim without republishing", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const result = await runtime.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const database = yield* Database;
					const notifier = yield* JournalNotifier;
					const preview = yield* PreviewTarget;

					yield* SeedThread;
					yield* preview.Register(register_command());

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						streams: yield* database.client.select().from(EventStreams),
					};

					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: "2026-07-14T22:00:03.000Z",
						thread_id: "thread_1",
					});

					const subscription = yield* notifier.Subscribe;
					const local_event_fiber = yield* preview.SlidingEvents.pipe(
						Stream.runHead,
						Effect.forkChild,
					);
					const notification_fiber = yield* PubSub.take(subscription).pipe(
						Effect.forkChild,
					);

					yield* Effect.yieldNow;

					const error = yield* preview.Register(register_command()).pipe(Effect.flip);
					const local_event = yield* Fiber.join(local_event_fiber).pipe(
						Effect.timeoutOption("50 millis"),
					);
					const notification = yield* Fiber.join(notification_fiber).pipe(
						Effect.timeoutOption("50 millis"),
					);
					const after = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						streams: yield* database.client.select().from(EventStreams),
					};

					return { after, before, error, local_event, notification };
				}),
			),
		);

		expect(result.error).toMatchObject({ code: "not_found", target_id: "preview_1" });
		expect(Option.isNone(result.local_event)).toBe(true);
		expect(Option.isNone(result.notification)).toBe(true);
		expect(result.after).toEqual(result.before);
	});

	it("replays a removed probe after restart without acquiring the unavailable adapter", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		let active_probes = 0;
		let probe_calls = 0;
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Effect.acquireRelease(
					Effect.sync(() => {
						active_probes += 1;
						probe_calls += 1;
					}),
					() =>
						Effect.sync(() => {
							active_probes -= 1;
						}),
				).pipe(
					Effect.as({
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					}),
				),
		});
		const first = make_runtime(database_path, probe);
		const accepted_probe = await first.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const preview = yield* PreviewTarget;

					yield* SeedThread;
					yield* preview.Register(register_command());
					const accepted = yield* preview.Probe(probe_command());

					yield* preview.Remove(remove_command());

					return accepted;
				}),
			),
		);

		await dispose_runtime(first);

		const second = make_runtime(database_path);
		const result = await second.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const preview = yield* PreviewTarget;
					const event_fiber = yield* preview.SlidingEvents.pipe(
						Stream.runHead,
						Effect.forkChild,
					);

					yield* Effect.yieldNow;

					const duplicate = yield* preview.Probe(probe_command());
					const local_event = yield* Fiber.join(event_fiber).pipe(
						Effect.timeoutOption("50 millis"),
					);
					const changed = yield* preview
						.Probe(probe_command("workspace_2"))
						.pipe(Effect.flip);

					return { changed, duplicate, local_event };
				}),
			),
		);

		expect(result.duplicate).toEqual({ event: accepted_probe.event, status: "duplicate" });
		expect(result.changed.code).toBe("conflict");
		expect(Option.isNone(result.local_event)).toBe(true);
		expect(probe_calls).toBe(1);
		expect(active_probes).toBe(0);
	});

	it("executes one adapter call for the same concurrent probe across two runtimes", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let probe_calls = 0;
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Effect.gen(function* () {
					probe_calls += 1;
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);

					return {
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					};
				}),
		});
		const preview_options = {
			probe_lease_ms: 1_000,
			probe_poll_interval_ms: 5,
			probe_timeout_ms: 500,
		} satisfies PreviewTargetOptions;
		const first = make_runtime(database_path, probe, { preview: preview_options });

		await first.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				yield* SeedThread;
				yield* preview.Register(register_command());
			}),
		);

		const second = make_runtime(database_path, probe, { preview: preview_options });
		const RunProbe = (runtime: typeof first) =>
			runtime.runPromise(
				Effect.scoped(
					Effect.flatMap(PreviewTarget, (preview) => preview.Probe(probe_command())),
				),
			);
		const first_probe = RunProbe(first);

		await Effect.runPromise(Deferred.await(started));

		const second_probe = RunProbe(second);

		await Effect.runPromise(Effect.sleep("30 millis"));

		expect(probe_calls).toBe(1);

		await Effect.runPromise(Deferred.succeed(release, undefined));

		const results = await Promise.all([first_probe, second_probe]);
		const rows = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					claims: yield* database.client.select().from(PreviewTargetProbeClaims),
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
				};
			}),
		);

		expect(results.map(({ status }) => status).toSorted()).toEqual(["accepted", "duplicate"]);
		expect(probe_calls).toBe(1);
		expect(rows.claims).toEqual([]);
		expect(rows.commands).toHaveLength(2);
		expect(rows.events).toHaveLength(2);
		expect(rows.streams).toEqual([{ last_sequence: 2, stream_id: "thread:thread_1" }]);
	});

	it("releases failed and timed out probes without journal evidence", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const failed_probe = Layer.succeed(PreviewHealthProbe, {
			Probe: (target) =>
				Effect.fail(
					new PreviewHealthProbeError({ reason: "failed", target_id: target.target_id }),
				),
		});
		const first = make_runtime(database_path, failed_probe, {
			preview: {
				probe_lease_ms: 200,
				probe_poll_interval_ms: 5,
				probe_timeout_ms: 50,
			},
		});
		const failure = await first.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const preview = yield* PreviewTarget;

					yield* SeedThread;
					yield* preview.Register(register_command());

					return yield* preview
						.Probe(probe_command("workspace_1", "probe_failure"))
						.pipe(Effect.flip);
				}),
			),
		);
		const claims_after_failure = await first.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.select().from(PreviewTargetProbeClaims),
			),
		);

		await dispose_runtime(first);

		const timed_out_probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () => Effect.never,
		});
		const second = make_runtime(database_path, timed_out_probe, {
			preview: {
				probe_lease_ms: 200,
				probe_poll_interval_ms: 5,
				probe_timeout_ms: 20,
			},
		});
		const timeout = await second.runPromise(
			Effect.scoped(
				Effect.flatMap(PreviewTarget, (preview) =>
					preview.Probe(probe_command("workspace_1", "probe_timeout")).pipe(Effect.flip),
				),
			),
		);
		const rows = await second.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					claims: yield* database.client.select().from(PreviewTargetProbeClaims),
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
				};
			}),
		);

		expect(failure.code).toBe("health_probe");
		expect(timeout.code).toBe("health_probe");
		expect(claims_after_failure).toEqual([]);
		expect(rows.claims).toEqual([]);
		expect(rows.commands.map(({ payload_type }) => payload_type)).toEqual([
			"preview.target.register",
		]);
		expect(rows.events).toHaveLength(1);
		expect(rows.streams).toEqual([{ last_sequence: 1, stream_id: "thread:thread_1" }]);
	});

	it("takes over an expired probe claim after restart with a new owner", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const metadata_now = { value: "2026-07-14T22:00:00.000Z" };
		const command = probe_command("workspace_1", "probe_takeover");
		const first = make_runtime(database_path, UnavailablePreviewHealthProbeLive, {
			metadata: make_metadata_layer("backend_probe_owner_1", metadata_now),
		});
		const first_result = await first.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const preview = yield* PreviewTarget;
				const repository = yield* PreviewTargetRepository;

				yield* SeedThread;
				yield* preview.Register(register_command());

				const claim = yield* repository.ClaimProbe(command, 1_000);

				return {
					claim,
					rows: yield* database.client.select().from(PreviewTargetProbeClaims),
				};
			}),
		);

		await dispose_runtime(first);

		metadata_now.value = "2026-07-14T22:00:02.000Z";

		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let probe_calls = 0;
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Effect.gen(function* () {
					probe_calls += 1;
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);

					return {
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					};
				}),
		});
		const second = make_runtime(database_path, probe, {
			metadata: make_metadata_layer("backend_probe_owner_2", metadata_now),
			preview: {
				probe_lease_ms: 1_000,
				probe_poll_interval_ms: 5,
				probe_timeout_ms: 500,
			},
		});
		const takeover = second.runPromise(
			Effect.scoped(Effect.flatMap(PreviewTarget, (preview) => preview.Probe(command))),
		);

		await Effect.runPromise(Deferred.await(started));

		const active_claim = await second.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.select().from(PreviewTargetProbeClaims),
			),
		);

		await Effect.runPromise(Deferred.succeed(release, undefined));

		const accepted = await takeover;
		const remaining_claims = await second.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.select().from(PreviewTargetProbeClaims),
			),
		);

		expect(first_result.claim._tag).toBe("Acquired");
		expect(first_result.rows).toMatchObject([{ owner_instance_id: "backend_probe_owner_1" }]);
		expect(active_claim).toMatchObject([{ owner_instance_id: "backend_probe_owner_2" }]);
		expect(accepted.status).toBe("accepted");
		expect(probe_calls).toBe(1);
		expect(remaining_claims).toEqual([]);
	});

	it("rejects changed intent against a live probe claim before adapter work", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let probe_calls = 0;
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Effect.gen(function* () {
					probe_calls += 1;
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);

					return {
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					};
				}),
		});
		const preview_options = {
			probe_lease_ms: 1_000,
			probe_poll_interval_ms: 5,
			probe_timeout_ms: 500,
		} satisfies PreviewTargetOptions;
		const first = make_runtime(database_path, probe, { preview: preview_options });

		await first.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				yield* SeedThread;
				yield* preview.Register(register_command());
			}),
		);

		const second = make_runtime(database_path, probe, { preview: preview_options });
		const owner = first.runPromise(
			Effect.scoped(
				Effect.flatMap(PreviewTarget, (preview) => preview.Probe(probe_command())),
			),
		);

		await Effect.runPromise(Deferred.await(started));

		const conflict = await second.runPromise(
			Effect.scoped(
				Effect.flatMap(PreviewTarget, (preview) =>
					preview.Probe(probe_command("workspace_2")).pipe(Effect.flip),
				),
			),
		);
		const competing_commands = await second.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				const register = yield* preview
					.Register(register_command("probe_1"))
					.pipe(Effect.flip);
				const remove = yield* preview.Remove(remove_command("probe_1")).pipe(Effect.flip);

				return { register, remove };
			}),
		);

		expect(conflict.code).toBe("conflict");
		expect(competing_commands.register.code).toBe("conflict");
		expect(competing_commands.remove.code).toBe("conflict");
		expect(probe_calls).toBe(1);

		await Effect.runPromise(Deferred.succeed(release, undefined));

		expect((await owner).status).toBe("accepted");
	});

	it("prevents an in-flight probe from committing after erasure starts", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Deferred.succeed(started, undefined).pipe(
					Effect.andThen(Deferred.await(release)),
					Effect.as({
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					}),
				),
		});
		const runtime = make_runtime(database_path, probe, {
			preview: {
				probe_lease_ms: 1_000,
				probe_poll_interval_ms: 5,
				probe_timeout_ms: 500,
			},
		});

		await runtime.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				yield* SeedThread;
				yield* preview.Register(register_command());
			}),
		);

		const outcome = runtime.runPromise(
			Effect.scoped(
				Effect.flatMap(PreviewTarget, (preview) =>
					preview
						.Probe(probe_command("workspace_1", "probe_erasure"))
						.pipe(Effect.result),
				),
			),
		);

		await Effect.runPromise(Deferred.await(started));

		const claims_before_erasure = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				yield* database.client.insert(ThreadErasureClaims).values({
					claimed_at: "2026-07-14T22:00:03.000Z",
					thread_id: "thread_1",
				});

				return yield* database.client.select().from(PreviewTargetProbeClaims);
			}),
		);

		await Effect.runPromise(Deferred.succeed(release, undefined));

		const result = await outcome;
		const rows = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;

				return {
					claims: yield* database.client.select().from(PreviewTargetProbeClaims),
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
				};
			}),
		);

		expect(claims_before_erasure).toHaveLength(1);
		expect(result._tag).toBe("Failure");
		if (result._tag === "Failure") {
			expect(result.failure.code).toBe("not_found");
		}
		expect(rows.claims).toEqual([]);
		expect(rows.commands.map(({ payload_type }) => payload_type)).toEqual([
			"preview.target.register",
		]);
		expect(rows.events).toHaveLength(1);
		expect(rows.streams).toEqual([{ last_sequence: 1, stream_id: "thread:thread_1" }]);
	});

	it("rejects a stale probe result after its target is replaced", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const probe = Layer.succeed(PreviewHealthProbe, {
			Probe: () =>
				Deferred.succeed(started, undefined).pipe(
					Effect.andThen(Deferred.await(release)),
					Effect.as({
						latency_ms: 1,
						message: Option.none<string>(),
						status: "healthy" as const,
						status_code: Option.some(200),
					}),
				),
		});
		const preview_options = {
			probe_lease_ms: 1_000,
			probe_poll_interval_ms: 5,
			probe_timeout_ms: 500,
		} satisfies PreviewTargetOptions;
		const first = make_runtime(database_path, probe, { preview: preview_options });

		await first.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				yield* SeedThread;
				yield* preview.Register(register_command());
			}),
		);

		const second = make_runtime(database_path, probe, { preview: preview_options });
		const outcome = first.runPromise(
			Effect.scoped(
				Effect.flatMap(PreviewTarget, (preview) =>
					preview
						.Probe(probe_command("workspace_1", "probe_replaced"))
						.pipe(Effect.result),
				),
			),
		);

		await Effect.runPromise(Deferred.await(started));

		await second.runPromise(
			Effect.gen(function* () {
				const preview = yield* PreviewTarget;

				yield* preview.Remove(remove_command("remove_before_replace"));
				yield* preview.Register(
					register_command("register_replacement", "http://localhost:4173/app"),
				);
			}),
		);

		await Effect.runPromise(Deferred.succeed(release, undefined));

		const result = await outcome;
		const rows = await second.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const preview = yield* PreviewTarget;

				return {
					claims: yield* database.client.select().from(PreviewTargetProbeClaims),
					commands: yield* database.client.select().from(JournalCommands),
					events: yield* database.client.select().from(JournalEvents),
					streams: yield* database.client.select().from(EventStreams),
					targets: yield* preview.List({
						project_id: "project_1",
						workspace_id: "workspace_1",
					}),
				};
			}),
		);

		expect(result._tag).toBe("Failure");
		if (result._tag === "Failure") {
			expect(result.failure.code).toBe("unavailable");
		}
		expect(rows.claims).toEqual([]);
		expect(rows.commands.map(({ payload_type }) => payload_type)).toEqual([
			"preview.target.register",
			"preview.target.remove",
			"preview.target.register",
		]);
		expect(rows.events).toHaveLength(3);
		expect(rows.streams).toEqual([{ last_sequence: 3, stream_id: "thread:thread_1" }]);
		expect(rows.targets).toMatchObject([
			{
				state: "registered",
				url: "http://localhost:4173/app",
			},
		]);
	});

	it("maps corrupt replay evidence to a source-safe invariant error", async () => {
		const database_path = await Effect.runPromise(
			MakeDatabasePath.pipe(Effect.provide(NodeFileSystem.layer)),
		);
		const runtime = make_runtime(database_path);
		const error = await runtime.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const preview = yield* PreviewTarget;

				yield* SeedThread;
				yield* preview.Register(register_command());
				yield* database.client.update(JournalEvents).set({ payload_json: "not-json" });

				return yield* preview.Register(register_command()).pipe(Effect.flip);
			}),
		);

		expect(error.code).toBe("invariant");
	});
});
