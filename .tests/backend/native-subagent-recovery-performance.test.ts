import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import { RecoverNativeSubagents } from "../../modules/backend/src/orchestration/internal/native-subagent-recovery";

type Binding = {
	readonly agent_path: string | null;
	readonly assignment_id: string;
	readonly assignment_role: string | null;
	readonly root_run_id: string;
	readonly thread_id: string | null;
};

type Pending = {
	readonly observation_id: string;
	readonly root_run_id: string;
	readonly thread_id: string | null;
};

const MeasureRecovery = (input: {
	readonly bindings: ReadonlyArray<Binding>;
	readonly pending: ReadonlyArray<Pending>;
	readonly transcripts: ReadonlyArray<{
		readonly observation_id: string;
		readonly root_run_id: string;
		readonly thread_id: string | null;
	}>;
}) => {
	let select_count = 0;
	const writes: Array<{ readonly assignment_id: string; readonly role: string }> = [];
	const events: Array<string> = [];
	const client = {
		select: () => {
			select_count += 1;
			const rows =
				select_count === 1
					? input.bindings
					: select_count === 2
						? input.pending
						: input.transcripts;
			return {
				from: () => ({
					leftJoin: () => ({
						leftJoin: () => ({ orderBy: () => Effect.succeed(rows) }),
						orderBy: () => Effect.succeed(rows),
						where: () => ({ orderBy: () => Effect.succeed(rows) }),
					}),
					where: () => ({ orderBy: () => Effect.succeed(rows) }),
				}),
			};
		},
		update: () => ({
			set: ({ role }: { readonly role: string }) => ({
				where: () =>
					Effect.sync(() => {
						writes.push({ assignment_id: "mismatched", role });
					}),
			}),
		}),
	};

	return Effect.runPromise(
		RecoverNativeSubagents(
			{
				agent_name_catalog: { Names: Effect.succeed(["Ada"]) },
				database: { client },
			} as never,
			{
				ConsumeTerminalTranscript: (observation_id) =>
					Effect.sync(() => events.push(`transcript:${observation_id}`)),
				RecordPending: (observation_id) =>
					Effect.sync(() => events.push(`record:${observation_id}`)),
				ReconcileRoot: (root_run_id) =>
					Effect.sync(() => events.push(`reconcile:${root_run_id}`)),
				RecoverTranscripts: Effect.sync(() => events.push("recover-transcripts")),
			},
		),
	).then(() => ({ events, select_count, writes }));
};

describe("native subagent recovery query bounds", () => {
	it("skips steady-state role writes, reuses binding roots, and preserves pending sequence", async () => {
		const result = await MeasureRecovery({
			bindings: [
				{
					agent_path: "researcher",
					assignment_id: "assignment-a",
					assignment_role: "Researcher",
					root_run_id: "root-a",
					thread_id: "thread-a",
				},
				{
					agent_path: "reviewer",
					assignment_id: "assignment-b",
					assignment_role: "Stale role",
					root_run_id: "root-b",
					thread_id: "thread-b",
				},
				{
					agent_path: "planner",
					assignment_id: "assignment-c",
					assignment_role: null,
					root_run_id: "root-missing-assignment",
					thread_id: "thread-c",
				},
				{
					agent_path: "reviewer",
					assignment_id: "assignment-d",
					assignment_role: "Reviewer",
					root_run_id: "root-b",
					thread_id: "thread-b",
				},
			],
			pending: [
				{ observation_id: "pending-a-1", root_run_id: "root-a", thread_id: "thread-a" },
				{ observation_id: "pending-a-2", root_run_id: "root-a", thread_id: "thread-a" },
				{
					observation_id: "pending-created",
					root_run_id: "root-created",
					thread_id: "thread-c",
				},
			],
			transcripts: [
				{ observation_id: "transcript-1", root_run_id: "root-a", thread_id: "thread-a" },
			],
		});

		expect(result.select_count).toBe(3);
		expect(result.writes).toEqual([{ assignment_id: "mismatched", role: "Reviewer" }]);
		expect(result.events).toEqual([
			"record:pending-a-1",
			"reconcile:root-a",
			"record:pending-a-2",
			"reconcile:root-a",
			"record:pending-created",
			"reconcile:root-created",
			"transcript:transcript-1",
			"recover-transcripts",
			"reconcile:root-b",
			"reconcile:root-missing-assignment",
		]);
	});

	it("does not let a blocked root delay another root while retaining same-root order", () =>
		Effect.runPromise(
			Effect.gen(function* () {
				const root_a_started = yield* Deferred.make<void>();
				const release_root_a = yield* Deferred.make<void>();
				const root_b_reconciled = yield* Deferred.make<void>();
				const events: Array<string> = [];
				let select_count = 0;
				const pending = [
					{ observation_id: "a-1", root_run_id: "root-a", thread_id: "thread-a" },
					{ observation_id: "a-2", root_run_id: "root-a", thread_id: "thread-a" },
					{
						observation_id: "same-thread-1",
						root_run_id: "root-a-2",
						thread_id: "thread-a",
					},
					{ observation_id: "b-1", root_run_id: "root-b", thread_id: "thread-b" },
				];
				const client = {
					select: () => {
						select_count += 1;
						const rows = select_count === 1 ? [] : select_count === 2 ? pending : [];
						return {
							from: () => ({
								leftJoin: () => ({
									leftJoin: () => ({ orderBy: () => Effect.succeed(rows) }),
									orderBy: () => Effect.succeed(rows),
									where: () => ({ orderBy: () => Effect.succeed(rows) }),
								}),
								where: () => ({ orderBy: () => Effect.succeed(rows) }),
							}),
						};
					},
				};
				const recovery = RecoverNativeSubagents(
					{
						agent_name_catalog: { Names: Effect.succeed(["Ada"]) },
						database: { client },
					} as never,
					{
						ConsumeTerminalTranscript: () => Effect.void,
						RecordPending: (observation_id) =>
							Effect.sync(() => events.push(`record:${observation_id}`)).pipe(
								Effect.andThen(
									observation_id === "a-1"
										? Deferred.succeed(root_a_started, undefined).pipe(
												Effect.andThen(Deferred.await(release_root_a)),
											)
										: Effect.void,
								),
							),
						ReconcileRoot: (root_run_id) =>
							Effect.sync(() => events.push(`reconcile:${root_run_id}`)).pipe(
								Effect.tap(() =>
									root_run_id === "root-b"
										? Deferred.succeed(root_b_reconciled, undefined)
										: Effect.void,
								),
							),
						RecoverTranscripts: Effect.void,
					},
				);
				const fiber = yield* recovery.pipe(Effect.forkChild({ startImmediately: true }));

				yield* Deferred.await(root_a_started);
				yield* Deferred.await(root_b_reconciled);

				expect(events).toContain("record:b-1");
				expect(events).toContain("reconcile:root-b");
				expect(events).not.toContain("record:a-2");
				expect(events).not.toContain("record:same-thread-1");
				expect(events.filter((event) => event.endsWith(":root-a"))).toEqual([]);

				yield* Deferred.succeed(release_root_a, undefined);
				yield* Fiber.join(fiber);

				expect(select_count).toBe(3);
				expect(
					events.filter(
						(event) =>
							event === "record:a-1" ||
							event === "record:a-2" ||
							event === "reconcile:root-a",
					),
				).toEqual(["record:a-1", "reconcile:root-a", "record:a-2", "reconcile:root-a"]);
				expect(events.indexOf("record:same-thread-1")).toBeGreaterThan(
					events.lastIndexOf("reconcile:root-a"),
				);
			}),
		));

	it("retries only uncommitted observations after another thread commits through a failure", () =>
		Effect.runPromise(
			Effect.gen(function* () {
				const other_thread_reconciled = yield* Deferred.make<void>();
				const committed = new Set<string>();
				const attempted: Array<string> = [];
				const published: Array<string> = [];
				const reconciled: Array<string> = [];
				const pending: ReadonlyArray<Pending> = [
					{ observation_id: "a-committed", root_run_id: "root-a", thread_id: "thread-a" },
					{ observation_id: "a-fails", root_run_id: "root-a", thread_id: "thread-a" },
					{ observation_id: "a-later", root_run_id: "root-a-2", thread_id: "thread-a" },
					{ observation_id: "b-committed", root_run_id: "root-b", thread_id: "thread-b" },
				];

				const MakeRecovery = (fail_before_commit: boolean) => {
					let select_count = 0;
					const client = {
						select: () => {
							select_count += 1;
							const rows =
								select_count === 1
									? []
									: select_count === 2
										? pending.filter(
												({ observation_id }) =>
													!committed.has(observation_id),
											)
										: [];
							return {
								from: () => ({
									leftJoin: () => ({
										leftJoin: () => ({ orderBy: () => Effect.succeed(rows) }),
										orderBy: () => Effect.succeed(rows),
										where: () => ({ orderBy: () => Effect.succeed(rows) }),
									}),
									where: () => ({ orderBy: () => Effect.succeed(rows) }),
								}),
							};
						},
					};

					return RecoverNativeSubagents(
						{
							agent_name_catalog: { Names: Effect.succeed(["Ada"]) },
							database: { client },
						} as never,
						{
							ConsumeTerminalTranscript: () => Effect.void,
							RecordPending: (observation_id) =>
								Effect.sync(() => attempted.push(observation_id)).pipe(
									Effect.andThen(
										fail_before_commit && observation_id === "a-fails"
											? Deferred.await(other_thread_reconciled).pipe(
													Effect.andThen(
														Effect.fail(
															new Error("pre-commit failure"),
														),
													),
												)
											: Effect.sync(() => {
													committed.add(observation_id);
													published.push(observation_id);
												}),
									),
								),
							ReconcileRoot: (root_run_id) =>
								Effect.sync(() => reconciled.push(root_run_id)).pipe(
									Effect.tap(() =>
										root_run_id === "root-b"
											? Deferred.succeed(other_thread_reconciled, undefined)
											: Effect.void,
									),
								),
							RecoverTranscripts: Effect.void,
						},
					);
				};

				const first = yield* MakeRecovery(true).pipe(Effect.exit);
				expect(first._tag).toBe("Failure");
				expect(new Set(attempted)).toEqual(
					new Set(["a-committed", "a-fails", "b-committed"]),
				);
				expect(committed).toEqual(new Set(["a-committed", "b-committed"]));
				expect(attempted).not.toContain("a-later");

				const retry_attempt_offset = attempted.length;
				const retry_publish_offset = published.length;
				const retry_reconcile_offset = reconciled.length;
				yield* MakeRecovery(false);

				expect(attempted.slice(retry_attempt_offset)).toEqual(["a-fails", "a-later"]);
				expect(published.slice(retry_publish_offset)).toEqual(["a-fails", "a-later"]);
				expect(reconciled.slice(retry_reconcile_offset)).toEqual(["root-a", "root-a-2"]);
				expect(committed).toEqual(
					new Set(["a-committed", "a-fails", "a-later", "b-committed"]),
				);
			}),
		));
});
