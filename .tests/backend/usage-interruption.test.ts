import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { Engine, EngineQuotaWindow } from "@artisan/engines";

import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import { ThreadContinuationRepository } from "../../modules/backend/src/persistence/thread-continuation/contracts";
import {
	JournalEvents,
	OrchestrationCoordinators,
	OrchestrationOutbox,
	OrchestrationRuns,
	SessionDefaults,
	Threads,
	UsageInterruptions,
} from "../../modules/backend/src/persistence/tables";
import {
	UsageInterruptionService,
	usage_interruption_continuation_text,
} from "../../modules/backend/src/persistence/usage-interruption/service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-usage-interruption-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const SeedRun = Effect.gen(function* () {
	const database = yield* Database;
	const now = "2026-08-14T10:00:00.000Z";
	yield* database.client.insert(Threads).values({
		created_at: now,
		thread_id: "thread_usage",
		title: "Usage",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationCoordinators).values({
		active_run_id: "run_usage",
		agent_id: "agent_usage",
		created_at: now,
		display_name: "Primary coordinator",
		engine_id: "codex",
		role: "primary",
		thread_id: "thread_usage",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_usage",
		created_at: now,
		engine_id: "codex",
		model_id: "gpt-5.6-sol",
		run_id: "run_usage",
		status: "running",
		thread_id: "thread_usage",
		updated_at: now,
		working_directory: "C:\\workspace",
	});
});

const UsageEngine = (windows: ReadonlyArray<EngineQuotaWindow>, engine_id = "codex") =>
	({
		Descriptor: { display_name: engine_id, id: engine_id },
		Usage: Effect.succeed({ authentication: { state: "authenticated" }, windows }),
	}) as unknown as Engine;

const SeedInterruption = (
	state: "scheduled" | "awaiting_decision" = "scheduled",
	limit_id: string | null = "primary",
) =>
	Effect.gen(function* () {
		const database = yield* Database;
		yield* SeedRun;
		yield* database.client.run(
			"UPDATE orchestration_runs SET status = 'failed' WHERE run_id = 'run_usage'",
		);
		yield* database.client.insert(UsageInterruptions).values({
			affected_model_id: null,
			alternatives_json: "[]",
			auto_continue: true,
			cancelled_at: null,
			continuation_command_id: null,
			continued_at: null,
			created_at: "2026-08-14T10:00:00.000Z",
			evidence_refreshed_at: "2026-08-14T10:00:00.000Z",
			failed_at: null,
			interruption_id: "usage-interruption:run_usage",
			limit_id,
			limit_label: null,
			limit_scope: "unknown",
			provider_code: "rate_limit_exceeded",
			resets_at: "2000-01-01T00:00:00.000Z",
			resume_not_before: "2000-01-01T00:00:00.000Z",
			revision: 0,
			source_agent_id: "agent_usage",
			source_engine_id: "codex",
			source_model_id: "gpt-5.6-sol",
			source_run_id: "run_usage",
			state,
			target_engine_id: null,
			target_model_id: null,
			target_run_id: null,
			thread_id: "thread_usage",
			updated_at: "2026-08-14T10:00:00.000Z",
		});
	});

describe("usage interruptions", () => {
	it("records the first exact usage failure once and rebuild-safe projects its card", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedRun;
					const repository = yield* OrchestrationRepository;
					const observation = {
						_tag: "native_action" as const,
						action: "usage limit",
						artisan_run_id: "run_usage",
						error_ref: {
							affected_model_id: "gpt-5.6-sol",
							artisan_code: "AE-PROVIDER-201",
							limit_id: "primary",
							limit_scope: "unknown" as const,
							provider_code: "rate_limit_exceeded",
							resets_at: "2026-08-14T12:00:00.000Z",
						},
						observation_id: "usage_failure",
						raw: { engine_id: "codex", frame: { private: true }, transport: "test" },
						sequence: 0,
					};
					yield* repository.RecordObservation(observation);
					yield* repository.RecordObservation(observation);
					const database = yield* Database;
					const rows = yield* database.client.select().from(UsageInterruptions);
					const conversation = yield* ConversationReadModel;
					return { rows, snapshot: yield* conversation.ReadSnapshot("thread_usage") };
				}),
			);

			expect(result.rows).toHaveLength(1);
			expect(result.rows[0]).toMatchObject({
				auto_continue: true,
				limit_id: "primary",
				state: "scheduled",
			});
			expect(result.rows[0]?.alternatives_json).not.toContain("private");
			expect(result.snapshot.status).toBe("available");
			if (result.snapshot.status !== "available") throw new Error("snapshot unavailable");
			const item = result.snapshot.snapshot.items.find(
				(entry) => entry.type === "usage_interruption",
			);
			expect(item).toMatchObject({ lifecycle: "active", type: "usage_interruption" });
		} finally {
			await runtime.dispose();
		}
	});

	it("captures a disabled default and does not schedule an unknown reset", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const row = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedRun;
					const database = yield* Database;
					yield* database.client.insert(SessionDefaults).values({
						auto_continue_usage_limits: false,
						defaults_id: 1,
						permission: "supervised",
						updated_at: "2026-08-14T10:00:00.000Z",
					});
					const repository = yield* OrchestrationRepository;
					yield* repository.RecordObservation({
						_tag: "process_diagnostic",
						artisan_run_id: "run_usage",
						error_ref: { artisan_code: "AE-PROVIDER-201" },
						level: "error",
						message: "depleted",
						observation_id: "usage_failure_no_reset",
						raw: { engine_id: "codex", frame: {}, transport: "test" },
						sequence: 0,
					});
					return (yield* database.client.select().from(UsageInterruptions))[0];
				}),
			);
			expect(row).toMatchObject({
				auto_continue: false,
				resume_not_before: null,
				state: "awaiting_decision",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("persists an unknown-reset poll and restart recovery never launches without capacity", async () => {
		const database_path = await MakePath();
		const first_runtime = make_backend_runtime({ database_path, migrations_path });
		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedRun;
					const repository = yield* OrchestrationRepository;
					yield* repository.RecordObservation({
						_tag: "process_diagnostic",
						artisan_run_id: "run_usage",
						error_ref: { artisan_code: "AE-PROVIDER-201", limit_id: "primary" },
						level: "error",
						message: "depleted",
						observation_id: "usage_unknown_reset",
						raw: { engine_id: "codex", frame: {}, transport: "test" },
						sequence: 0,
					});
					const database = yield* Database;
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'failed' WHERE run_id = 'run_usage'",
					);
					yield* database.client.run(
						"UPDATE usage_interruptions SET resume_not_before = '2000-01-01T00:00:00.000Z'",
					);
					yield* database.client.run(
						"UPDATE threads SET pinned = 1 WHERE thread_id = 'thread_usage'",
					);
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const restarted = make_backend_runtime({
			database_path,
			engines: [
				UsageEngine([
					{ id: "primary", kind: "session", percent_used: 100, scope: "unknown" },
				]),
			],
			migrations_path,
		});
		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					return {
						interruptions: yield* database.client.select().from(UsageInterruptions),
						outbox: yield* database.client.select().from(OrchestrationOutbox),
					};
				}),
			);
			expect(result.outbox).toHaveLength(0);
			expect(result.interruptions[0]).toMatchObject({
				auto_continue: true,
				resets_at: null,
				state: "scheduled",
				target_run_id: null,
			});
			expect(result.interruptions[0]?.resume_not_before).not.toBeNull();
			expect(result.interruptions[0]?.resume_not_before).not.toBe("2000-01-01T00:00:00.000Z");
		} finally {
			await restarted.dispose();
		}
	});

	it("does not launch while any relevant non-model allowance remains depleted", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [
				UsageEngine([
					{ id: "primary", kind: "session", percent_used: 10, scope: "unknown" },
					{
						id: "secondary",
						kind: "weekly",
						percent_used: 100,
						resets_at: "2099-08-14T12:00:00.000Z",
						scope: "unknown",
					},
				]),
			],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("scheduled", null);
					const service = yield* UsageInterruptionService;
					yield* service.ScanDue;
					const database = yield* Database;
					return {
						interruptions: yield* database.client.select().from(UsageInterruptions),
						outbox: yield* database.client.select().from(OrchestrationOutbox),
					};
				}),
			);
			expect(result.outbox).toHaveLength(0);
			expect(result.interruptions[0]).toMatchObject({
				resets_at: "2099-08-14T12:00:00.000Z",
				resume_not_before: "2099-08-14T12:00:00.000Z",
				state: "scheduled",
				target_run_id: null,
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("waits for the exact source run to settle as failed before launching", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [
				UsageEngine([
					{ id: "primary", kind: "session", percent_used: 10, scope: "unknown" },
				]),
			],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption();
					const database = yield* Database;
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'running' WHERE run_id = 'run_usage'",
					);
					const service = yield* UsageInterruptionService;
					yield* service.ScanDue;
					const while_running = yield* database.client.select().from(OrchestrationOutbox);
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'failed' WHERE run_id = 'run_usage'",
					);
					yield* service.ScanDue;
					return {
						after_failed: yield* database.client.select().from(OrchestrationOutbox),
						while_running,
					};
				}),
			);
			expect(result.while_running).toHaveLength(0);
			expect(result.after_failed).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("claims a verified due continuation once and settles its exact target lifecycle", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [
				UsageEngine([
					{ id: "primary", kind: "session", percent_used: 10, scope: "unknown" },
				]),
			],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption();
					const service = yield* UsageInterruptionService;
					const continuation = yield* ThreadContinuationRepository;
					const repository = yield* OrchestrationRepository;
					const database = yield* Database;
					yield* database.client.run(
						"UPDATE orchestration_coordinators SET policy_context_window = '[1m]', policy_reasoning_effort = 'max', policy_service_tier = 'fast' WHERE thread_id = 'thread_usage'",
					);
					yield* database.client.run(
						"UPDATE orchestration_runs SET status = 'running' WHERE run_id = 'run_usage'",
					);
					yield* repository.RecordObservation({
						_tag: "run_terminal",
						artisan_run_id: "run_usage",
						observation_id: "source_terminal_evidence",
						raw: { engine_id: "codex", frame: {}, transport: "test" },
						sequence: 0,
						state: "failed",
					});
					yield* service.ScanDue;
					yield* service.ScanDue;
					const before = (yield* database.client.select().from(UsageInterruptions))[0];
					if (before?.target_run_id === null || before?.target_run_id === undefined)
						throw new Error("target missing");
					const ready = yield* continuation.IsDispatchReady(before.target_run_id);
					const target_events = (yield* database.client
						.select()
						.from(JournalEvents)).filter(
						(event) => event.run_id === before.target_run_id,
					);
					const conversation = yield* ConversationReadModel;
					const snapshot = yield* conversation.ReadSnapshot("thread_usage");
					const coordinator = (yield* database.client
						.select()
						.from(OrchestrationCoordinators))[0];
					yield* database.client.run(
						`UPDATE orchestration_runs SET native_thread_id = 'native-source', native_resume_json = '{"native_thread_id":"native-source"}' WHERE run_id = 'run_usage'`,
					);
					yield* database.client.run(
						`UPDATE orchestration_outbox SET status = 'dispatching' WHERE run_id = '${before.target_run_id}'`,
					);
					const native_preparation = yield* continuation.PrepareLaunch(
						before.target_run_id,
						{
							_tag: "native",
							request_id: before.continuation_command_id ?? "",
							source_run_id: before.source_run_id,
							...(before.target_model_id === null
								? {}
								: { target_model_id: before.target_model_id }),
						},
					);
					yield* service.MarkTargetContinued(before.target_run_id);
					const continued = (yield* database.client.select().from(UsageInterruptions))[0];
					yield* database.client.run(
						`UPDATE usage_interruptions SET state = 'launching' WHERE target_run_id = '${before.target_run_id}'`,
					);
					yield* service.MarkTargetFailed(before.target_run_id);
					const failed = (yield* database.client.select().from(UsageInterruptions))[0];
					return {
						before,
						continued,
						coordinator,
						failed,
						native_preparation,
						outbox: yield* database.client.select().from(OrchestrationOutbox),
						ready,
						snapshot,
						target_events,
					};
				}),
			);
			expect(result.outbox).toHaveLength(1);
			expect(result.before).toMatchObject({ state: "launching" });
			expect(result.ready).toBe(true);
			expect(result.native_preparation).toBe("prepared");
			expect(result.target_events).toHaveLength(1);
			expect(result.target_events[0]?.event_type).toBe("usage.interruption.updated");
			expect(result.coordinator).toMatchObject({
				policy_context_window: "[1m]",
				policy_reasoning_effort: "max",
				policy_service_tier: "fast",
			});
			expect(result.snapshot.status).toBe("available");
			if (result.snapshot.status !== "available") throw new Error("snapshot unavailable");
			expect(
				result.snapshot.snapshot.items.some(
					(item) =>
						item.type === "user_message" &&
						item.text.includes(usage_interruption_continuation_text),
				),
			).toBe(false);
			expect(result.continued).toMatchObject({ state: "continued" });
			expect(result.failed).toMatchObject({ state: "failed" });
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an exact resolve command without a second revision or event", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("awaiting_decision");
					const service = yield* UsageInterruptionService;
					const command = {
						kind: "command" as const,
						message_id: "resolve-duplicate",
						origin: "frontend" as const,
						payload: {
							action: { enabled: false, type: "set_auto_continue" as const },
							expected_revision: 0,
							interruption_id: "usage-interruption:run_usage",
							type: "usage.interruption.resolve" as const,
						},
						protocol_version: 1 as const,
						schema_version: 1 as const,
						sent_at: "2026-08-14T10:00:00.000Z",
						thread_id: "thread_usage",
					};
					const first = yield* service.Resolve(command);
					const duplicate = yield* service.Resolve(command);
					const database = yield* Database;
					return {
						duplicate,
						first,
						row: (yield* database.client.select().from(UsageInterruptions))[0],
					};
				}),
			);
			expect(result.first.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.journal_sequence).toBe(result.first.journal_sequence);
			expect(result.duplicate.events).toHaveLength(1);
			expect(result.row?.revision).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an accepted continuation without re-reading changed provider usage", async () => {
		let depleted = false;
		const engine = {
			Descriptor: { display_name: "Codex", id: "codex" },
			Usage: Effect.sync(() => ({
				authentication: { state: "authenticated" as const },
				windows: [
					{
						id: "primary",
						kind: "session" as const,
						percent_used: depleted ? 100 : 10,
						scope: "unknown" as const,
					},
				],
			})),
		} as unknown as Engine;
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [engine],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("awaiting_decision");
					const service = yield* UsageInterruptionService;
					const command = {
						kind: "command" as const,
						message_id: "continue-duplicate",
						origin: "frontend" as const,
						payload: {
							action: {
								target_engine_id: "codex",
								target_model_id: "gpt-5.6-sol",
								type: "continue" as const,
							},
							expected_revision: 0,
							interruption_id: "usage-interruption:run_usage",
							type: "usage.interruption.resolve" as const,
						},
						protocol_version: 1 as const,
						schema_version: 1 as const,
						sent_at: "2026-08-14T10:00:00.000Z",
						thread_id: "thread_usage",
					};
					const first = yield* service.Resolve(command);
					depleted = true;
					const replay = yield* service.Resolve(command);
					return { first, replay };
				}),
			);
			expect(result.first.status).toBe("accepted");
			expect(result.replay.status).toBe("duplicate");
			expect(result.replay.journal_sequence).toBe(result.first.journal_sequence);
		} finally {
			await runtime.dispose();
		}
	});

	it("accepts only a freshly verified separate model allowance", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [
				UsageEngine([
					{
						id: "spark",
						kind: "weekly",
						label: "GPT 5.3 Codex Spark",
						percent_used: 10,
						scope: "model",
					},
				]),
			],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("awaiting_decision");
					const service = yield* UsageInterruptionService;
					const Base = (message_id: string, target_model_id: string) =>
						service.Resolve({
							kind: "command",
							message_id,
							origin: "frontend",
							payload: {
								action: {
									target_engine_id: "codex",
									target_model_id,
									type: "continue",
								},
								expected_revision: 0,
								interruption_id: "usage-interruption:run_usage",
								type: "usage.interruption.resolve",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-14T10:00:00.000Z",
							thread_id: "thread_usage",
						});
					const unverified = yield* Base("switch-unverified", "made-up").pipe(
						Effect.exit,
					);
					const verified = yield* Base("switch-verified", "gpt-5.3-codex-spark");
					return { unverified, verified };
				}),
			);
			expect(result.unverified._tag).toBe("Failure");
			expect(result.verified.status).toBe("accepted");
		} finally {
			await runtime.dispose();
		}
	});

	it("offers another Claude model when only Fable's provider bucket is depleted", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [
				UsageEngine(
					[
						{ id: "five_hour", kind: "session", percent_used: 10, scope: "shared" },
						{ id: "seven_day", kind: "weekly", percent_used: 20, scope: "shared" },
						{
							id: "seven_day:fable",
							kind: "weekly",
							label: "Fable",
							percent_used: 100,
							scope: "model",
						},
					],
					"claude",
				),
			],
			migrations_path,
		});
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("awaiting_decision", "seven_day_fable");
					const database = yield* Database;
					yield* database.client.run(
						"UPDATE usage_interruptions SET evidence_refreshed_at = NULL, limit_scope = 'unknown', limit_label = NULL, affected_model_id = NULL, source_engine_id = 'claude', source_model_id = 'claude-fable-5'",
					);
					yield* database.client.run(
						"UPDATE orchestration_runs SET engine_id = 'claude', model_id = 'claude-fable-5' WHERE run_id = 'run_usage'",
					);
					yield* database.client.run(
						"UPDATE orchestration_coordinators SET engine_id = 'claude', policy_model = 'claude-fable-5' WHERE thread_id = 'thread_usage'",
					);
					const service = yield* UsageInterruptionService;
					yield* service.RefreshPendingEvidence;
					const refreshed = (yield* database.client.select().from(UsageInterruptions))[0];
					if (refreshed === undefined) throw new Error("interruption missing");
					const receipt = yield* service.Resolve({
						kind: "command",
						message_id: "switch-fable-to-opus",
						origin: "frontend",
						payload: {
							action: {
								target_engine_id: "claude",
								target_model_id: "claude-opus-5",
								type: "continue",
							},
							expected_revision: refreshed.revision,
							interruption_id: "usage-interruption:run_usage",
							type: "usage.interruption.resolve",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-14T10:00:00.000Z",
						thread_id: "thread_usage",
					});
					return { receipt, refreshed };
				}),
			);
			expect(JSON.parse(result.refreshed.alternatives_json)).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						display_name: "Claude Opus 5",
						model_id: "claude-opus-5",
					}),
				]),
			);
			expect(result.refreshed).toMatchObject({
				affected_model_id: "claude-fable-5",
				limit_label: "Fable",
				limit_scope: "model",
			});
			expect(result.receipt.status).toBe("accepted");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects stale resolution revisions and projects supersession", async () => {
		const runtime = make_backend_runtime({ database_path: await MakePath(), migrations_path });
		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					yield* SeedInterruption("awaiting_decision");
					const service = yield* UsageInterruptionService;
					const stale = yield* service
						.Resolve({
							kind: "command",
							message_id: "stale",
							origin: "frontend",
							payload: {
								action: { type: "cancel" },
								expected_revision: 1,
								interruption_id: "usage-interruption:run_usage",
								type: "usage.interruption.resolve",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-14T10:00:00.000Z",
							thread_id: "thread_usage",
						})
						.pipe(Effect.exit);
					yield* service.CancelSuperseded("thread_usage", "run_new");
					const database = yield* Database;
					return {
						row: (yield* database.client.select().from(UsageInterruptions))[0],
						stale,
					};
				}),
			);
			expect(result.stale._tag).toBe("Failure");
			expect(result.row).toMatchObject({ state: "cancelled" });
		} finally {
			await runtime.dispose();
		}
	});
});
