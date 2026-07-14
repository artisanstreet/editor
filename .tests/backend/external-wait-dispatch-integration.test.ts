import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Latch, Layer, PubSub, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { Engine, EngineOpenInput, EngineRun } from "@artisan/engines";
import type { HostedGitCheck, HostedGitPullRequestLookup } from "@artisan/protocol";
import {
	BuildExternalWaitBaseline,
	ExternalWaitCoordinator,
	ExternalWaitDispatcher,
	ExternalWaitDispatchScheduler,
	ExternalWaitRepository,
	ExternalWaitScheduler,
	GitProvider,
	make_backend_runtime,
	make_git_provider_registry_layer,
	ProjectRepository,
} from "@artisan/backend";

import type { GitProviderPullRequestTargetRead } from "../../modules/backend/src/git-provider/git-provider";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalNotifier } from "../../modules/backend/src/persistence/journal-notifier";
import {
	OrchestrationCoordinators,
	OrchestrationRuns,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const initial_now = "2026-07-14T15:00:00.000Z";
const observed_now = "2026-07-14T15:00:15.000Z";
const head = "b".repeat(40);

const repository = {
	host: "github.com",
	name: "editor",
	owner: "artisan",
	provider_id: "github",
} as const;

const target = {
	branch: "main",
	expected_head_commit: head,
	pull_request_number: 7,
	pull_request_origin: {
		native_id: "pr_7",
		provider_id: "github",
		resource_kind: "pull_request",
	},
	repository,
} as const;

interface ClockState {
	value: string;
}

interface ProviderState {
	readonly calls: Array<GitProviderPullRequestTargetRead>;
	readonly lookup: HostedGitPullRequestLookup;
}

function check(state: HostedGitCheck["state"]): HostedGitCheck {
	return {
		annotations: [],
		annotations_truncated: false,
		name: "build",
		origin: {
			native_id: "check_1",
			provider_id: "github",
			resource_kind: "check_run",
		},
		required: true,
		state,
	};
}

function lookup(check_state: HostedGitCheck["state"]): HostedGitPullRequestLookup {
	return {
		association: {
			_tag: "matched",
			freshness: "current",
			pull_request: {
				base_branch: "main",
				base_commit: "a".repeat(40),
				checks: [check(check_state)],
				checks_total: 1,
				checks_truncated: false,
				draft: false,
				head_branch: "main",
				head_commit: head,
				mergeability: "mergeable",
				number: 7,
				origin: target.pull_request_origin,
				requested_reviewers: [],
				requested_reviewers_truncated: false,
				review_decision: "none",
				review_threads: [],
				review_threads_total: 0,
				review_threads_truncated: false,
				reviews: [],
				reviews_total: 0,
				reviews_truncated: false,
				state: "open",
				title: "External wait",
				web_url: "https://github.com/artisan/editor/pull/7",
			},
		},
		branch: "main",
		expected_head_commit: head,
		repository,
	};
}

function make_provider(state: ProviderState): typeof GitProvider.Service {
	return {
		Clone: () => Effect.die("unused"),
		Descriptor: {
			capabilities: [
				{ _tag: "available", capability: "read_reviews" },
				{ _tag: "available", capability: "read_ci" },
			],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () => Effect.die("unused"),
		Inspect: Effect.die("unused"),
		PrepareClone: () => Effect.die("unused"),
		ReadPullRequestTarget: (input) =>
			Effect.sync(() => {
				state.calls.push(input);

				return state.lookup;
			}),
	};
}

function make_engine(drift_after_materialization: boolean) {
	const inputs: Array<EngineOpenInput> = [];
	const opened_latch = Latch.makeUnsafe();
	let resume_reads = 0;
	const capabilities = {
		...Object.fromEntries(
			[
				"approval",
				"auth",
				"cancel",
				"close",
				"events",
				"global_guidance",
				"harness_context",
				"model_selection",
				"native_tools",
				"probe",
				"question",
				"raw_frames",
				"start",
				"steer",
				"subagents",
			].map((name) => [name, { state: "supported" as const }]),
		),
		get resume() {
			resume_reads += 1;

			return {
				state:
					drift_after_materialization && resume_reads > 1
						? ("unsupported" as const)
						: ("supported" as const),
			};
		},
	} as Engine["Descriptor"]["capabilities"];
	const Open = (input: EngineOpenInput) =>
		Effect.gen(function* () {
			const native_thread_id =
				input._tag === "resume"
					? input.resume_token.native_thread_id
					: `native:${input.artisan_run_id}`;

			inputs.push(input);
			yield* opened_latch.open;

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: Effect.never,
				Events: Stream.never,
				native_thread_id,
				resume_token: { native_thread_id },
				Send: () => Effect.void,
			} satisfies EngineRun;
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Controlled engine",
				id: "codex",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("unused"),
		} satisfies Engine,
		inputs,
		opened_latch,
		resume_reads: () => resume_reads,
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-external-wait-dispatch-"));

	directories.push(directory);

	return join(directory, "artisan.db");
}

function metadata_layer(clock: ClockState) {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "external_wait_dispatch_integration",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_dispatch_${++identifier}`),
		Now: Effect.sync(() => clock.value),
	});
}

function runtime(
	database_path: string,
	clock: ClockState,
	provider_state: ProviderState,
	engine: Engine,
) {
	const observation_scheduler = Layer.succeed(ExternalWaitScheduler, {
		Schedule: () => Effect.never,
	});
	const dispatch_scheduler = Layer.succeed(ExternalWaitDispatchScheduler, {
		Schedule: () => Effect.never,
	});

	return make_backend_runtime({
		database_path,
		engines: [engine],
		external_wait_dispatch_scheduler: dispatch_scheduler,
		external_wait_scheduler: observation_scheduler,
		git_provider_registry: make_git_provider_registry_layer([
			{ hosts: ["github.com"], provider: make_provider(provider_state) },
		]),
		migrations_path,
		runtime_metadata: metadata_layer(clock),
	});
}

const PrepareProviderWake = (clock: ClockState) =>
	Effect.gen(function* () {
		const coordinator = yield* ExternalWaitCoordinator;
		const database = yield* Database;
		const external_waits = yield* ExternalWaitRepository;
		const projects = yield* ProjectRepository;
		const registered = yield* projects.RegisterHosted({
			canonical_root: "C:/artisan",
			display_name: "Artisan",
			hosted_origin: {
				canonical_host: "github.com",
				clone_url: "https://github.com/artisan/editor.git",
				fetch_url: "https://github.com/artisan/editor.git",
				name: "editor",
				native_id: "repository_1",
				owner: "artisan",
				provider_id: "github",
				push_url: "https://github.com/artisan/editor.git",
				remote_name: "origin",
				selected_account_login: "sander",
				web_url: "https://github.com/artisan/editor",
			},
		});
		const token = JSON.stringify({
			native_thread_id: "native:source",
			opaque_checkpoint: "provider-owned",
		});
		const baseline = yield* BuildExternalWaitBaseline({
			gates: [{ _tag: "required_checks_terminal" }],
			lookup: lookup("running"),
			target,
		});

		if (baseline._tag !== "usable") {
			return yield* Effect.die("Expected a usable external wait baseline");
		}

		yield* database.client.insert(Threads).values({
			created_at: initial_now,
			primary_project_id: registered.project.project.project_id,
			primary_project_json: JSON.stringify(registered.project.project),
			thread_id: "thread_1",
			title: "External wait",
			title_source: "initial",
			updated_at: initial_now,
		});
		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_1",
			created_at: initial_now,
			engine_id: "codex",
			native_resume_json: token,
			native_thread_id: "native:source",
			run_id: "source_run",
			status: "running",
			thread_id: "thread_1",
			updated_at: initial_now,
			working_directory: "C:/artisan",
		});
		yield* database.client.insert(OrchestrationCoordinators).values({
			active_run_id: "source_run",
			agent_id: "agent_1",
			created_at: initial_now,
			display_name: "Primary coordinator",
			engine_id: "codex",
			native_resume_json: token,
			native_thread_id: "native:source",
			role: "primary",
			thread_id: "thread_1",
			updated_at: initial_now,
		});
		yield* external_waits.Register({
			baseline: baseline.baseline,
			owner: {
				_tag: "thread_run",
				agent_id: "agent_1",
				engine_id: "codex",
				run_id: "source_run",
			},
			project_id: registered.project.project.project_id,
			request: {
				expected_head_commit: head,
				gates: [{ _tag: "required_checks_terminal" }],
				pull_request_number: 7,
				source_run_id: "source_run",
				workspace_id: registered.project.workspace_id,
			},
			request_fingerprint: "b".repeat(64),
			source_command: { message_id: "wait_command", sent_at: initial_now },
			target,
			thread_id: "thread_1",
			wait_id: "wait_1",
		});
		yield* external_waits.MarkSourceClosed({
			now: observed_now,
			wait_id: "wait_1",
		});
		yield* Effect.sync(() => {
			clock.value = observed_now;
		});
		yield* coordinator.RunOnce;
	});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("external wait dispatch integration", () => {
	it("carries a provider wake through atomic materialization into Engine.Open", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = { calls: [], lookup: lookup("passed") } satisfies ProviderState;
		const controlled = make_engine(false);
		const instance = runtime(
			await make_database_path(),
			clock,
			provider_state,
			controlled.engine,
		);

		try {
			await instance.runPromise(PrepareProviderWake(clock));
			const snapshot = await instance.runPromise(
				Effect.gen(function* () {
					const dispatcher = yield* ExternalWaitDispatcher;
					const external_waits = yield* ExternalWaitRepository;

					yield* dispatcher.RunOnce;
					yield* controlled.opened_latch.await;

					const result = yield* external_waits.Query({ thread_id: "thread_1" });

					return result.snapshots[0];
				}),
			);

			expect(provider_state.calls).toHaveLength(1);
			expect(controlled.inputs).toEqual([
				expect.objectContaining({
					_tag: "resume",
					next_text: "External checks reached a terminal state. Continue the task.",
					resume_token: {
						native_thread_id: "native:source",
						opaque_checkpoint: "provider-owned",
					},
				}),
			]);
			expect(snapshot?.state).toMatchObject({ _tag: "woken", mode: "native_resume" });
		} finally {
			await instance.dispose();
		}
	});

	it("fails the chosen native continuation when resume support drifts before open", async () => {
		const clock = { value: initial_now } satisfies ClockState;
		const provider_state = { calls: [], lookup: lookup("passed") } satisfies ProviderState;
		const controlled = make_engine(true);
		const instance = runtime(
			await make_database_path(),
			clock,
			provider_state,
			controlled.engine,
		);

		try {
			await instance.runPromise(PrepareProviderWake(clock));
			const result = await instance.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const database = yield* Database;
						const dispatcher = yield* ExternalWaitDispatcher;
						const external_waits = yield* ExternalWaitRepository;
						const notifier = yield* JournalNotifier;
						const subscription = yield* notifier.Subscribe;

						yield* dispatcher.RunOnce;
						yield* PubSub.take(subscription);
						yield* PubSub.take(subscription);
						yield* PubSub.take(subscription);

						const runs = yield* database.client.select().from(OrchestrationRuns);
						const query = yield* external_waits.Query({ thread_id: "thread_1" });

						return { query, runs };
					}),
				),
			);
			const continuation = result.runs.find((run) => run.open_mode === "resume");

			expect(controlled.resume_reads()).toBe(2);
			expect(controlled.inputs).toEqual([]);
			expect(continuation?.status).toBe("failed");
			expect(result.query.snapshots[0]?.state).toMatchObject({
				_tag: "woken",
				mode: "native_resume",
			});
		} finally {
			await instance.dispose();
		}
	});
});
