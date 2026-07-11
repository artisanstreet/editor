import { Effect, Exit, Layer, Scope } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadListItem } from "@artisan/protocol";

import {
	ThreadMetadataRepository,
	type ThreadMetadataAcceptance,
	type ThreadMetadataRefinementIntent,
} from "../../modules/backend/src/threads/thread-metadata-repository";
import {
	make_thread_metadata_refiner_test_layer,
	ThreadMetadataRefiner,
	type ThreadMetadataRefinerInput,
	ThreadMetadataRefinerLive,
	type ThreadMetadataRefinement,
	type ThreadMetadataRefinementRequest,
} from "../../modules/backend/src/threads/thread-metadata-refiner";
import {
	make_thread_metadata_refinement_worker_layer,
	ThreadMetadataRefinementWorker,
} from "../../modules/backend/src/threads/thread-metadata-refinement-worker";

const projection = (thread_id: string, versions = [0, 0]): ThreadListItem => ({
	activity_version: versions[0]!,
	affinity_version: 0,
	created_at: "2026-07-10T17:00:00.000Z",
	current_goal: "Ship metadata",
	last_activity_at: "2026-07-10T18:00:00.000Z",
	linked_projects: [],
	live_status: "Working",
	metadata_version: versions[1]!,
	pinned: false,
	project_affinity_scores: [],
	project_locked: false,
	primary_project: undefined,
	rename_suggestion: "Metadata worker",
	rehome_suggestion: undefined,
	thread_id,
	title: "Thread",
	title_locked: false,
	title_source: "initial",
	updated_at: "2026-07-10T18:00:00.000Z",
});

const request = (
	thread_id: string,
	projection_value = projection(thread_id),
	trigger: ThreadMetadataRefinementRequest["trigger"] = "user_message",
): ThreadMetadataRefinementRequest => ({
	projection: projection_value,
	source_event_id: `event_${thread_id}`,
	thread_id,
	trigger,
	recent_user_text: ["Ship the metadata worker"],
	recent_activity: ["User sent a message"],
	recent_files: ["modules/backend/src/threads/thread-metadata-refiner.ts"],
	recent_artifacts: ["diff: thread metadata"],
});

const make_test_layer = (
	refine: (input: ThreadMetadataRefinerInput) => Effect.Effect<ThreadMetadataRefinement, unknown>,
	accepted: Array<ThreadMetadataRefinementIntent>,
	options?: { readonly max_pending?: number },
) => {
	const repository = Layer.succeed(ThreadMetadataRepository, {
		Accept: () => Effect.die("Frontend metadata commands are not used by this worker"),
		Refine: (intent) =>
			Effect.sync(() => {
				accepted.push(intent);
				return {} as ThreadMetadataAcceptance;
			}),
		WasRefined: () => Effect.succeed(false),
	});

	return make_thread_metadata_refinement_worker_layer(options).pipe(
		Layer.provideMerge(repository),
		Layer.provideMerge(make_thread_metadata_refiner_test_layer(refine)),
	);
};

describe("thread metadata refinement worker", () => {
	it("does not propose a title for a manually locked projection", async () => {
		const locked = {
			...projection("thread_1"),
			title_locked: true,
			title_source: "manual" as const,
		};
		const refinement = await Effect.runPromise(
			Effect.gen(function* () {
				const refiner = yield* ThreadMetadataRefiner;
				return yield* refiner.Refine({
					...request("thread_1", locked),
				});
			}).pipe(Effect.provide(ThreadMetadataRefinerLive)),
		);

		expect(refinement).not.toHaveProperty("title");
	});

	it("coalesces bursts and keeps the latest request for a thread", async () => {
		const accepted: ThreadMetadataRefinementIntent[] = [];
		const first = request("thread_1");
		const latest = request("thread_1", projection("thread_1", [9, 4]), "run_completed");
		await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ThreadMetadataRefinementWorker;
				expect(yield* service.Submit(first)).toBe("queued");
				expect(yield* service.Submit(latest)).toBe("coalesced");
				yield* service.WaitForIdle;
			}).pipe(
				Effect.provide(
					make_test_layer(
						(input) => Effect.succeed({ live_status: input.trigger }),
						accepted,
					),
				),
			),
		);

		expect(accepted).toHaveLength(1);
		expect(accepted[0]).toMatchObject({
			operation_id: "metadata-refine:event_thread_1",
			source_event_id: "event_thread_1",
		});
		expect(accepted[0]!.payload).toMatchObject({
			basis_activity_version: 9,
			basis_metadata_version: 4,
			live_status: "run_completed",
		});
	});

	it("bounds context before invoking the provider", async () => {
		const seen: ThreadMetadataRefinerInput[] = [];
		const values = Array.from({ length: 12 }, (_, index) => ` value_${index} `);
		const accepted: ThreadMetadataRefinementIntent[] = [];
		await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ThreadMetadataRefinementWorker;
				yield* service.Submit({ ...request("thread_1"), recent_user_text: values });
				yield* service.WaitForIdle;
			}).pipe(
				Effect.provide(
					make_test_layer(
						(input) =>
							Effect.sync(() => {
								seen.push(input);
								return { live_status: "Working" };
							}),
						accepted,
					),
				),
			),
		);

		expect(seen[0]!.recent_user_text).toHaveLength(8);
		expect(seen[0]!.recent_user_text[0]).toBe("value_4");
	});

	it("isolates provider failures and continues with another thread", async () => {
		const accepted: ThreadMetadataRefinementIntent[] = [];
		await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ThreadMetadataRefinementWorker;
				expect(yield* service.Submit(request("thread_1"))).toBe("queued");
				yield* service.WaitForIdle;
				expect(yield* service.Submit(request("thread_2"))).toBe("queued");
				yield* service.WaitForIdle;
			}).pipe(
				Effect.provide(
					make_test_layer(
						(input) =>
							input.projection.thread_id === "thread_1"
								? Effect.fail(new Error("provider unavailable"))
								: Effect.succeed({
										current_goal: "New goal",
										live_status: "Complete",
									}),
						accepted,
					),
				),
			),
		);

		expect(accepted).toHaveLength(1);
		expect(accepted[0]!.thread_id).toBe("thread_2");
	});

	it("isolates provider defects without terminating the drain fiber", async () => {
		const accepted: ThreadMetadataRefinementIntent[] = [];

		await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ThreadMetadataRefinementWorker;

				yield* service.Submit(request("thread_1"));
				yield* service.WaitForIdle;
				yield* service.Submit(request("thread_2"));
				yield* service.WaitForIdle;
			}).pipe(
				Effect.provide(
					make_test_layer(
						(input) =>
							input.projection.thread_id === "thread_1"
								? Effect.die("provider defect")
								: Effect.succeed({ live_status: "Recovered" }),
						accepted,
					),
				),
			),
		);

		expect(accepted).toHaveLength(1);
		expect(accepted[0]!.thread_id).toBe("thread_2");
	});

	it("interrupts the consumer when its scope closes", async () => {
		const calls = yield_ref();
		const accepted: ThreadMetadataRefinementIntent[] = [];
		const layer = make_test_layer(
			() =>
				Effect.sync(() => {
					calls.increment();
					return { live_status: "Working" };
				}),
			accepted,
		);
		const scope = await Effect.runPromise(Scope.make());
		await Effect.runPromise(
			Effect.gen(function* () {
				const service = yield* ThreadMetadataRefinementWorker;
				yield* service.Submit(request("thread_1"));
			}).pipe(Effect.provide(layer), Scope.provide(scope)),
		);
		await Effect.runPromise(Scope.close(scope, Exit.void));
		const before = calls.value();
		await new Promise((resolve) => setTimeout(resolve, 10));
		expect(calls.value()).toBe(before);
	});
});

function yield_ref() {
	let value = 0;

	return {
		increment: () => {
			value += 1;
		},
		value: () => value,
	};
}
