import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime, Option, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { EventPayload } from "@artisan/protocol";

import { fileURLToPath } from "node:url";

import {
	ToolControlRepository,
	ToolControlRepositoryLive,
} from "../../modules/backend/src/tool-control/tool-control-repository";
import {
	ToolExecutionRepository,
	ToolExecutionRepositoryLive,
} from "../../modules/backend/src/tool-control/tool-execution-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalNotifier,
	JournalNotifierLive,
} from "../../modules/backend/src/persistence/journal-notifier";
import {
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ToolExecutionClaims,
	ToolInvocationPrivate,
	ToolInvocations,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	type ToolRegistration,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const clock = { value: "2026-07-16T12:00:00.000Z" };

const descriptor = {
	approval_policy: "automatic" as const,
	effect: "read" as const,
	input_schema: { properties: { query: { type: "string" } }, type: "object" },
	label: "Read workspace",
	revision: 1,
	source: "artisan" as const,
	summary: "Reads a bounded workspace view",
	tool_id: "workspace.read",
};

function registry_layer() {
	const Registration = (approval_policy: "automatic" | "required"): ToolRegistration => {
		const current_descriptor = {
			...descriptor,
			approval_policy,
			tool_id: approval_policy === "required" ? "workspace.replace" : "workspace.read",
		};

		return {
			adapter: {
				input_schema: current_descriptor.input_schema,
				Invoke: () => Effect.succeed({ ok: true }),
			},
			descriptor: current_descriptor,
			IsEligible: () => Effect.void,
			recovery_policy: approval_policy === "required" ? "outcome_unknown" : "retry",
		};
	};

	return make_tool_registry_layer([Registration("automatic"), Registration("required")]);
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-tool-execution-" });

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer(instance_id: string) {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++identifier}`),
		Now: Effect.sync(() => clock.value),
	});
}

function runtime(
	database_path: string,
	instance_id = "execution_one",
	notifier = JournalNotifierLive,
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(instance_id),
		notifier,
		registry_layer(),
		NodeCrypto.layer,
	);
	const services = Layer.merge(ToolControlRepositoryLive, ToolExecutionRepositoryLive);

	return ManagedRuntime.make(services.pipe(Layer.provideMerge(infrastructure)));
}

const InstallToolThreadDispatchState = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.run(`
		CREATE TABLE IF NOT EXISTS tool_thread_dispatch_state (
			thread_id text PRIMARY KEY REFERENCES threads(thread_id) ON DELETE CASCADE,
			admission_version integer NOT NULL DEFAULT 0,
			quiesced_at text,
			CONSTRAINT tool_thread_dispatch_state_admission_version_check
				CHECK (admission_version >= 0),
			CONSTRAINT tool_thread_dispatch_state_quiesced_at_check
				CHECK (
					quiesced_at IS NULL OR (
						strftime('%Y-%m-%dT%H:%M:%fZ', quiesced_at) IS quiesced_at
						AND substr(quiesced_at, 12, 2) BETWEEN '00' AND '23'
					)
				)
		)
	`);
});

interface ToolOwnership {
	readonly agent_id: string;
	readonly run_id: string;
	readonly thread_id: string;
}

const primary_ownership = {
	agent_id: "agent_1",
	run_id: "run_1",
	thread_id: "thread_1",
} satisfies ToolOwnership;

const SeedThreadWithOwnership = (ownership: ToolOwnership) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* InstallToolThreadDispatchState;
		yield* database.client.insert(Threads).values({
			created_at: clock.value,
			thread_id: ownership.thread_id,
			title: "Tool execution",
			title_source: "initial",
			updated_at: clock.value,
		});
		yield* database.client.run(`
			INSERT INTO orchestration_runs
			(run_id, thread_id, agent_id, engine_id, status, working_directory, created_at, updated_at)
			VALUES ('${ownership.run_id}', '${ownership.thread_id}', '${ownership.agent_id}', 'codex', 'running', 'C:/artisan', '${clock.value}', '${clock.value}')
		`);
	});

const SeedThread = SeedThreadWithOwnership(primary_ownership);

function request(
	request_id: string,
	approval_policy: "automatic" | "required" = "automatic",
	ownership: ToolOwnership = primary_ownership,
) {
	return {
		descriptor: {
			...descriptor,
			approval_policy,
			tool_id: approval_policy === "required" ? "workspace.replace" : "workspace.read",
		},
		recovery_policy:
			approval_policy === "required" ? ("outcome_unknown" as const) : ("retry" as const),
		request: {
			arguments: { query: "private-token" },
			context: ownership,
			request_id,
			tool: {
				revision: 1,
				tool_id: approval_policy === "required" ? "workspace.replace" : "workspace.read",
			},
		},
	};
}

const Prepare = (
	request_id: string,
	approval_policy: "automatic" | "required" = "automatic",
	ownership: ToolOwnership = primary_ownership,
) =>
	Effect.gen(function* () {
		const controls = yield* ToolControlRepository;
		const prepared = yield* controls.Prepare(
			request(request_id, approval_policy, ownership).request,
		);

		if (approval_policy === "required") {
			yield* controls.Decide({
				approval_id: prepared.approval!.approval_id,
				decision: "approved",
				decision_id: `decision_${request_id}`,
				thread_id: ownership.thread_id,
			});
		}

		return prepared.invocation.invocation_id;
	});

afterEach(async () => {
	clock.value = "2026-07-16T12:00:00.000Z";

	for (const directory of directories.splice(0)) {
		await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		);
	}
});

describe("ToolExecutionRepository", () => {
	it("claims automatic work, redacts private data from journals, and completes it", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("automatic_complete");
					const executions = yield* ToolExecutionRepository;
					const pending = yield* executions.ListPending;
					const claimed = Option.getOrThrow(
						yield* executions.ClaimPending(invocation_id),
					);
					const active = yield* executions.ActiveClaimsForThread("thread_1");
					const identity = {
						claim_token: claimed.claim_token,
						invocation_id,
					};

					yield* executions.MarkLaunchStarted(identity);
					yield* executions.MarkLaunchStarted(identity);
					const completed = yield* executions.SettleCompleted(identity, {
						secret_result: "private-result",
					});
					const duplicate = yield* executions.SettleCompleted(identity, {
						secret_result: "private-result",
					});
					const changed_settlement = yield* executions
						.SettleCompleted(identity, { secret_result: "changed-result" })
						.pipe(Effect.exit);
					const completed_read = Option.getOrThrow(
						yield* executions.ReadCompleted(request("automatic_complete").request),
					);
					const mismatched_read = yield* executions
						.ReadCompleted({
							...request("automatic_complete").request,
							arguments: { query: "different-private-token" },
						})
						.pipe(Effect.exit);
					const active_after = yield* executions.ActiveClaimsForThread("thread_1");
					const database = yield* Database;

					return {
						active,
						active_after,
						arguments_: claimed.arguments,
						changed_settlement,
						completed,
						completed_read,
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						mismatched_read,
						pending,
						private_rows: yield* database.client.select().from(ToolInvocationPrivate),
					};
				}),
			);

			expect(result.arguments_).toEqual({ query: "private-token" });
			expect(result.pending).toEqual([
				{ invocation_id: result.completed.invocation_id, thread_id: "thread_1" },
			]);
			expect(result.active).toBe(true);
			expect(result.active_after).toBe(false);
			expect(result.completed.state).toBe("completed");
			expect(result.duplicate).toEqual(result.completed);
			expect(result.changed_settlement._tag).toBe("Failure");
			expect(result.mismatched_read._tag).toBe("Failure");
			expect(JSON.stringify(result.mismatched_read)).not.toContain("private-result");
			expect(result.completed_read).toMatchObject({
				result: { secret_result: "private-result" },
			});
			expect(JSON.stringify(result.events)).not.toContain("private-token");
			expect(JSON.stringify(result.events)).not.toContain("private-result");
			expect(result.private_rows[0]?.result_json).toContain("private-result");
			for (const event of result.events) {
				expect(() =>
					Schema.decodeUnknownSync(EventPayload, { onExcessProperty: "error" })(
						JSON.parse(event.payload_json),
					),
				).not.toThrow();
			}
			expect(
				result.events
					.map((event) => JSON.parse(event.payload_json) as { state?: string })
					.filter((payload) => payload.state !== undefined)
					.map((payload) => payload.state),
			).toContain("running");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("executes an approved tool and publishes its approval projection", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("approved_execution", "required");
					const executions = yield* ToolExecutionRepository;
					const claimed = Option.getOrThrow(
						yield* executions.ClaimPending(invocation_id),
					);
					yield* executions.MarkLaunchStarted({
						claim_token: claimed.claim_token,
						invocation_id,
					});
					yield* executions.SettleFailed({
						claim_token: claimed.claim_token,
						invocation_id,
					});
					const database = yield* Database;

					return yield* database.client.select().from(JournalEvents);
				}),
			);

			expect(
				result.filter((event) => event.event_type === "tool.approval.updated"),
			).toHaveLength(4);
			expect(result.at(-1)?.event_type).toBe("tool.invocation.updated");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("fences stale lease tokens and supports failed settlement only after launch", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("fencing");
					const executions = yield* ToolExecutionRepository;
					const claimed = Option.getOrThrow(
						yield* executions.ClaimPending(invocation_id),
					);
					const before_launch = yield* executions
						.SettleFailed({ claim_token: claimed.claim_token, invocation_id })
						.pipe(Effect.exit);
					const stale = yield* executions
						.RenewLease({ claim_token: "claim_wrong", invocation_id })
						.pipe(Effect.exit);

					yield* executions.MarkLaunchStarted({
						claim_token: claimed.claim_token,
						invocation_id,
					});

					return {
						before_launch,
						failed: yield* executions.SettleFailed({
							claim_token: claimed.claim_token,
							invocation_id,
						}),
						stale,
					};
				}),
			);

			expect(result.before_launch._tag).toBe("Failure");
			expect(result.stale._tag).toBe("Failure");
			expect(result.failed.state).toBe("failed");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("rejects an idempotent launch acknowledgement after its lease expires", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("expired_launch_ack");
					const executions = yield* ToolExecutionRepository;
					const claimed = Option.getOrThrow(
						yield* executions.ClaimPending(invocation_id),
					);
					const identity = { claim_token: claimed.claim_token, invocation_id };

					yield* executions.MarkLaunchStarted(identity);
					clock.value = "2026-07-16T12:00:31.000Z";

					return yield* executions.MarkLaunchStarted(identity).pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("allows one of two runtimes to claim pending work", async () => {
		const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			MakeDatabasePath,
		);
		const first_runtime = runtime(database_path, "execution_one");
		const second_runtime = runtime(database_path, "execution_two");

		try {
			const invocation_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					return yield* Prepare("claim_race");
				}),
			);
			const results = await Promise.all([
				first_runtime.runPromise(
					ToolExecutionRepository.pipe(
						Effect.flatMap((repository) => repository.ClaimPending(invocation_id)),
					),
				),
				second_runtime.runPromise(
					ToolExecutionRepository.pipe(
						Effect.flatMap((repository) => repository.ClaimPending(invocation_id)),
					),
				),
			]);

			expect(results.filter(Option.isSome)).toHaveLength(1);
		} finally {
			await first_runtime.dispose();
			await second_runtime.dispose();
		}
	});

	it("classifies restart recovery, waiting ownership, quarantine, and abandonment", async () => {
		const database_path = await ManagedRuntime.make(NodeFileSystem.layer).runPromise(
			MakeDatabasePath,
		);
		const ownership = {
			no_launch: {
				agent_id: "agent_no_launch",
				run_id: "run_no_launch",
				thread_id: "thread_no_launch",
			},
			retry_launch: {
				agent_id: "agent_retry_launch",
				run_id: "run_retry_launch",
				thread_id: "thread_retry_launch",
			},
			unknown_launch: {
				agent_id: "agent_unknown_launch",
				run_id: "run_unknown_launch",
				thread_id: "thread_unknown_launch",
			},
			waiting: {
				agent_id: "agent_waiting",
				run_id: "run_waiting",
				thread_id: "thread_waiting",
			},
		} satisfies Readonly<Record<string, ToolOwnership>>;
		const first_runtime = runtime(database_path, "execution_one");
		const second_runtime = runtime(database_path, "execution_two");

		try {
			const ids = await first_runtime.runPromise(
				Effect.gen(function* () {
					yield* Effect.forEach(Object.values(ownership), SeedThreadWithOwnership, {
						discard: true,
					});
					const executions = yield* ToolExecutionRepository;
					const no_launch = yield* Prepare("no_launch", "automatic", ownership.no_launch);
					const retry_launch = yield* Prepare(
						"retry_launch",
						"automatic",
						ownership.retry_launch,
					);
					const unknown_launch = yield* Prepare(
						"unknown_launch",
						"required",
						ownership.unknown_launch,
					);
					const waiting = yield* Prepare("waiting", "automatic", ownership.waiting);
					const retry_claim = Option.getOrThrow(
						yield* executions.ClaimPending(retry_launch),
					);
					const unknown_claim = Option.getOrThrow(
						yield* executions.ClaimPending(unknown_launch),
					);
					yield* executions.MarkLaunchStarted({
						claim_token: retry_claim.claim_token,
						invocation_id: retry_launch,
					});
					yield* executions.MarkLaunchStarted({
						claim_token: unknown_claim.claim_token,
						invocation_id: unknown_launch,
					});
					Option.getOrThrow(yield* executions.ClaimPending(no_launch));
					Option.getOrThrow(yield* executions.ClaimPending(waiting));

					return { no_launch, retry_launch, unknown_launch, waiting };
				}),
			);
			const waiting = await second_runtime.runPromise(
				ToolExecutionRepository.pipe(
					Effect.flatMap((repository) => repository.ListRunning),
				),
			);
			clock.value = "2026-07-16T12:00:31.000Z";
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const executions = yield* ToolExecutionRepository;
					const dispatches = yield* executions.ListRunning;
					const recovered_no_launch = yield* executions.ClaimRecovery(ids.no_launch);
					const recovered_retry = yield* executions.ClaimRecovery(ids.retry_launch);
					yield* executions.QuarantineInterrupted(ids.unknown_launch);
					yield* executions.AbandonOwnedExecutions;
					const database = yield* Database;

					return {
						claims: yield* database.client.select().from(ToolExecutionClaims),
						dispatches,
						recovered_no_launch,
						recovered_retry,
						unknown: (yield* database.client.select().from(ToolInvocations)).find(
							(row) => row.invocation_id === ids.unknown_launch,
						),
					};
				}),
			);

			expect(waiting.find((item) => item.invocation_id === ids.waiting)?.recovery).toBe(
				"waiting",
			);
			expect(
				result.dispatches.find((item) => item.invocation_id === ids.no_launch)?.recovery,
			).toBe("recoverable");
			expect(Option.isSome(result.recovered_no_launch)).toBe(true);
			expect(Option.isSome(result.recovered_retry)).toBe(true);
			expect(result.unknown?.state).toBe("outcome_unknown");
			expect(result.claims.some((claim) => claim.owner_instance_id === "execution_two")).toBe(
				true,
			);
		} finally {
			await first_runtime.dispose();
			await second_runtime.dispose();
		}
	});

	it("fails closed for corrupt private data and fences erasure", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const corruption_id = yield* Prepare("corrupt");
					const erased_id = yield* Prepare("erased");
					const database = yield* Database;
					yield* database.client.run(
						`UPDATE tool_invocation_private SET arguments_digest = '${"a".repeat(64)}' WHERE invocation_id = '${corruption_id}'`,
					);
					const executions = yield* ToolExecutionRepository;
					const corrupt = yield* executions.ClaimPending(corruption_id).pipe(Effect.exit);
					yield* database.client
						.insert(ThreadErasureClaims)
						.values({ claimed_at: clock.value, thread_id: "thread_1" });
					const erased = yield* executions.ClaimPending(erased_id).pipe(Effect.exit);

					return { corrupt, erased };
				}),
			);

			expect(result.corrupt._tag).toBe("Failure");
			expect(JSON.stringify(result.corrupt)).not.toContain("private-token");
			expect(result.erased._tag).toBe("Failure");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("fails closed when the private request fingerprint no longer binds the admitted request", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("request_fingerprint_corrupt");
					const database = yield* Database;

					yield* database.client.run(
						`UPDATE tool_invocation_private SET request_fingerprint = '${"b".repeat(64)}' WHERE invocation_id = '${invocation_id}'`,
					);

					const executions = yield* ToolExecutionRepository;

					return yield* executions.ClaimPending(invocation_id).pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).not.toContain("private-token");
		} finally {
			await current_runtime.dispose();
		}
	});

	it("returns committed execution state when notifier publication defects", async () => {
		const current_runtime = runtime(
			await ManagedRuntime.make(NodeFileSystem.layer).runPromise(MakeDatabasePath),
			"execution_notifier",
			Layer.succeed(JournalNotifier, {
				Publish: () => Effect.die("notifier defect"),
				Subscribe: Effect.die("unused"),
			}),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedThread;
					const invocation_id = yield* Prepare("notifier_defect");
					const executions = yield* ToolExecutionRepository;
					const claimed = Option.getOrThrow(
						yield* executions.ClaimPending(invocation_id),
					);
					const identity = { claim_token: claimed.claim_token, invocation_id };

					yield* executions.MarkLaunchStarted(identity);

					return yield* executions.SettleCompleted(identity, { ok: true });
				}),
			);

			expect(result.state).toBe("completed");
		} finally {
			await current_runtime.dispose();
		}
	});
});
