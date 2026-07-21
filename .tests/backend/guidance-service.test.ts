import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GlobalGuidanceRepositoryLive } from "../../modules/backend/src/guidance/guidance-repository";
import {
	GlobalGuidanceService,
	make_global_guidance_service_layer,
} from "../../modules/backend/src/guidance/guidance-service";
import {
	GuidanceProviderRegistry,
	guidance_hash,
	make_codex_guidance_adapter,
	make_runtime_guidance_adapter,
	make_unsupported_guidance_adapter,
	normalize_guidance_content,
} from "../../modules/backend/src/guidance/provider-mirrors";
import { make_test_native_guidance_adapter } from "./guidance-test-adapter";
import {
	GuidanceFileStore,
	GuidanceFileStoreFailure,
	GuidanceFileStoreLive,
	type GuidanceConditionalWriteInput,
} from "../../modules/backend/src/guidance/file-store";
import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { Database } from "../../modules/backend/src/persistence/database";
import {
	GlobalGuidanceCanonical,
	GlobalGuidanceProviderSync,
	JournalCommands,
	JournalEvents,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

interface GuidancePaths {
	readonly backups: string;
	readonly canonical: string;
	readonly claude: string;
	readonly codex_agents: string;
	readonly codex_override: string;
	readonly database: string;
	readonly root: string;
}

interface RuntimeOptions {
	readonly fail_provider_write?: string;
	readonly mutate_before_replace?: {
		readonly path: string;
		readonly replacement: string;
		readonly when_content?: string;
	};
	readonly provider_mode?: "native" | "non_native";
	readonly registry?: Layer.Layer<GuidanceProviderRegistry>;
}

async function make_paths(): Promise<GuidancePaths> {
	const root = await mkdtemp(join(tmpdir(), "artisan-guidance-service-"));

	temporary_directories.push(root);

	return {
		backups: join(root, "backups"),
		canonical: join(root, "artisan", "GLOBAL.md"),
		claude: join(root, "claude", "CLAUDE.md"),
		codex_agents: join(root, "codex", "AGENTS.md"),
		codex_override: join(root, "codex", "override.md"),
		database: join(root, "artisan.db"),
		root,
	};
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "guidance_service_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T12:00:00.000Z"),
	});
}

function make_runtime(paths: GuidancePaths, options: RuntimeOptions = {}) {
	const metadata = make_metadata_layer();
	let mutated_before_replace = false;
	const file_store =
		options.fail_provider_write === undefined && options.mutate_before_replace === undefined
			? GuidanceFileStoreLive
			: Layer.effect(
					GuidanceFileStore,
					Effect.gen(function* () {
						const live = yield* GuidanceFileStore;

						return {
							...live,
							ReplaceAtomic: (input: GuidanceConditionalWriteInput) => {
								if (input.path === options.fail_provider_write) {
									return Effect.gen(function* () {
										const backup_path = yield* live.CopyToBackup(
											input.path,
											input.backups_directory,
											input.backup_name,
										);

										return yield* new GuidanceFileStoreFailure({
											backup_path,
											cause: new Error("injected provider write failure"),
											operation: "replace",
											path: input.path,
										});
									});
								}

								if (
									!mutated_before_replace &&
									input.path === options.mutate_before_replace?.path &&
									(options.mutate_before_replace.when_content === undefined ||
										input.content ===
											options.mutate_before_replace.when_content)
								) {
									mutated_before_replace = true;

									return Effect.promise(() =>
										writeFile(
											input.path,
											options.mutate_before_replace!.replacement,
											"utf8",
										),
									).pipe(Effect.andThen(live.ReplaceAtomic(input)));
								}

								return live.ReplaceAtomic(input);
							},
							WriteAtomic: (path: string, content: string) =>
								path === options.fail_provider_write
									? Effect.fail(
											new GuidanceFileStoreFailure({
												cause: new Error("injected provider write failure"),
												operation: "write",
												path,
											}),
										)
									: live.WriteAtomic(path, content),
						};
					}),
				).pipe(Layer.provide(GuidanceFileStoreLive));
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path: paths.database, migrations_path }),
		metadata,
		JournalNotifierLive,
	);
	const repository = GlobalGuidanceRepositoryLive.pipe(
		Layer.provideMerge(JournalStoreLive.pipe(Layer.provideMerge(infrastructure))),
		Layer.provideMerge(infrastructure),
	);
	const registry =
		options.registry ??
		(options.provider_mode === "non_native"
			? Layer.succeed(GuidanceProviderRegistry, {
					Providers: [
						make_runtime_guidance_adapter("codex"),
						make_unsupported_guidance_adapter("claude"),
					],
				})
			: Layer.effect(
					GuidanceProviderRegistry,
					Effect.gen(function* () {
						const codex = yield* make_codex_guidance_adapter(
							paths.codex_override,
							paths.codex_agents,
						);
						const claude = yield* make_test_native_guidance_adapter(
							"claude",
							paths.claude,
						);

						return { Providers: [codex, claude] };
					}),
				).pipe(Layer.provideMerge(file_store)));
	const service = make_global_guidance_service_layer({
		backups_directory: paths.backups,
		canonical_path: paths.canonical,
	}).pipe(
		Layer.provideMerge(repository),
		Layer.provideMerge(registry),
		Layer.provideMerge(file_store),
		Layer.provideMerge(metadata),
	);

	return ManagedRuntime.make(service);
}

function make_mutating_claude_registry(
	path: string,
	replacement: string,
	should_mutate: (discovery_count: number, current_content: string) => boolean,
) {
	let armed = false;
	let discovery_count = 0;
	let mutated = false;

	const registry = Layer.effect(
		GuidanceProviderRegistry,
		Effect.gen(function* () {
			const claude = yield* make_test_native_guidance_adapter("claude", path);
			const Discover = Effect.gen(function* () {
				if (armed) {
					discovery_count += 1;

					const current_content = yield* Effect.promise(() =>
						readFile(path, "utf8").catch(() => ""),
					);

					if (
						!mutated &&
						should_mutate(discovery_count, normalize_guidance_content(current_content))
					) {
						mutated = true;
						yield* Effect.promise(() => writeFile(path, replacement, "utf8"));
					}
				}

				return yield* claude.Discover;
			});

			return { Providers: [{ ...claude, Discover }] };
		}),
	).pipe(Layer.provide(GuidanceFileStoreLive));

	return {
		arm: () => {
			armed = true;
		},
		registry,
	};
}

function trace(message_id: string) {
	return {
		message_id,
		origin: "frontend" as const,
		sent_at: "2026-07-11T12:00:00.000Z",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("global guidance service", () => {
	it("creates an empty canonical source and syncs absent provider mirrors", async () => {
		const paths = await make_paths();
		const runtime = make_runtime(paths);

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					return yield* guidance.Initialize;
				}),
			);

			expect(snapshot).toMatchObject({
				candidates: [],
				content: "",
				metadata: {
					canonical: { byte_count: 0, status: "ready" },
					providers: [
						{ provider: "claude", status: "synced" },
						{ provider: "codex", status: "synced" },
					],
				},
			});
			expect(await readFile(paths.canonical, "utf8")).toBe("");
			expect(await readFile(paths.claude, "utf8")).toBe("");
			expect(await readFile(paths.codex_agents, "utf8")).toBe("");
		} finally {
			await runtime.dispose();
		}
	});

	it("imports one provider value and deduplicates identical first-run sources", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Use services.\r\n", "utf8");
		await writeFile(paths.claude, "Use services.\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					return yield* guidance.Initialize;
				}),
			);

			expect(snapshot.candidates).toEqual([]);
			expect(snapshot.content).toBe("Use services.\n");
			expect(snapshot.metadata.canonical).toMatchObject({
				selected_provider: "codex",
				status: "ready",
			});
			expect(
				snapshot.metadata.providers.every((provider) => provider.status === "synced"),
			).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("requires an explicit choice for unique values and backs up overwritten mirrors", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Codex value\n", "utf8");
		await writeFile(paths.claude, "Claude value\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;
					const initial = yield* guidance.Initialize;
					const selected = initial.candidates.find(
						(candidate) => candidate.provider === "codex",
					)!;
					const mutation = yield* guidance.Select({
						...trace("guidance_select_1"),
						content_hash: selected.content_hash,
						provider: "codex",
					});

					return { initial, mutation };
				}),
			);

			expect(result.initial.metadata.canonical.status).toBe("selection_required");
			expect(result.initial.candidates).toHaveLength(2);
			expect(result.mutation.snapshot.content).toBe("Codex value\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Codex value\n");
			expect((await readdir(paths.backups)).length).toBeGreaterThanOrEqual(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an accepted selection before consulting changed provider state", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Codex value\n", "utf8");
		await writeFile(paths.claude, "Claude value\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;
					const initial = yield* guidance.Initialize;
					const selected = initial.candidates.find(
						(candidate) => candidate.provider === "codex",
					)!;
					const accepted = yield* guidance.Select({
						...trace("guidance_selection_replay"),
						content_hash: selected.content_hash,
						provider: "codex",
					});

					yield* Effect.promise(() =>
						writeFile(paths.codex_agents, "Changed after selection\n", "utf8"),
					);

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const duplicate = yield* guidance.Select({
						...trace("guidance_selection_replay"),
						content_hash: selected.content_hash,
						provider: "codex",
						sent_at: "2026-07-11T12:01:00.000Z",
					});

					return {
						accepted,
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						duplicate,
						provider: yield* Effect.promise(() => readFile(paths.codex_agents, "utf8")),
					};
				}),
			);

			expect(result.accepted.acceptance.status).toBe("accepted");
			expect(result.duplicate.acceptance.status).toBe("duplicate");
			expect(result.after).toEqual(result.before);
			expect(result.canonical).toBe("Codex value\n");
			expect(result.provider).toBe("Changed after selection\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects selection once canonical guidance is initialized", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() =>
						writeFile(paths.codex_agents, "External drift\n", "utf8"),
					);
					yield* guidance.Get;

					const before = {
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejection = yield* guidance
						.Select({
							...trace("guidance_selection_after_initialization"),
							content_hash: guidance_hash("External drift\n"),
							provider: "codex",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);

					return {
						after: {
							canonical: yield* Effect.promise(() =>
								readFile(paths.canonical, "utf8"),
							),
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						provider: yield* Effect.promise(() => readFile(paths.codex_agents, "utf8")),
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceConflict",
				reason: "selection_not_required",
			});
			expect(result.after).toEqual(result.before);
			expect(result.provider).toBe("External drift\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects a provider value that was not among the recorded selection candidates", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Candidate A\n", "utf8");
		await writeFile(paths.claude, "Candidate B\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() =>
						writeFile(paths.codex_agents, "Unpresented candidate\n", "utf8"),
					);

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejection = yield* guidance
						.Select({
							...trace("guidance_unpresented_selection"),
							content_hash: guidance_hash("Unpresented candidate\n"),
							provider: "codex",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceConflict",
				reason: "candidate_changed",
			});
			expect(result.after).toEqual(result.before);
			expect(await readFile(paths.codex_agents, "utf8")).toBe("Unpresented candidate\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects selection when an unselected provider changed after candidates were shown", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Candidate A\n", "utf8");
		await writeFile(paths.claude, "Candidate B\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;
					const initial = yield* guidance.Initialize;
					const selected = initial.candidates.find(
						(candidate) => candidate.provider === "codex",
					)!;

					yield* Effect.promise(() => writeFile(paths.claude, "Unreviewed C\n", "utf8"));

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejection = yield* guidance
						.Select({
							...trace("guidance_unselected_candidate_changed"),
							content_hash: selected.content_hash,
							provider: "codex",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceConflict",
				provider: "claude",
				reason: "candidate_changed",
			});
			expect(result.after).toEqual(result.before);
			expect(await readFile(paths.codex_agents, "utf8")).toBe("Candidate A\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Unreviewed C\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("detects drift and supports ignore, overwrite, and import without merging", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* guidance.Update({
						...trace("guidance_update_1"),
						content: "Canonical\n",
					});
					yield* Effect.promise(() => writeFile(paths.claude, "Claude drift\n", "utf8"));
					const drifted = yield* guidance.Get;
					const claude_drift = drifted.metadata.providers.find(
						(provider) => provider.provider === "claude",
					)!;
					const ignored = yield* guidance.ResolveDrift({
						...trace("guidance_ignore_1"),
						action: "ignore",
						observed_hash: claude_drift.observed_hash!,
						provider: "claude",
					});
					const overwritten = yield* guidance.ResolveDrift({
						...trace("guidance_overwrite_1"),
						action: "overwrite",
						observed_hash: claude_drift.observed_hash!,
						provider: "claude",
					});
					yield* Effect.promise(() =>
						writeFile(paths.codex_agents, "Imported drift\n", "utf8"),
					);
					const second_drift = yield* guidance.Get;
					const codex_drift = second_drift.metadata.providers.find(
						(provider) => provider.provider === "codex",
					)!;
					const imported = yield* guidance.ResolveDrift({
						...trace("guidance_import_1"),
						action: "import",
						observed_hash: codex_drift.observed_hash!,
						provider: "codex",
					});

					return { drifted, ignored, imported, overwritten };
				}),
			);

			expect(
				result.drifted.metadata.providers.find(
					(provider) => provider.provider === "claude",
				),
			).toMatchObject({ status: "drift_detected" });
			expect(
				result.ignored.snapshot.metadata.providers.find(
					(provider) => provider.provider === "claude",
				),
			).toMatchObject({
				ignored_drift_hash: expect.any(String),
				status: "drift_detected",
			});
			expect(await readFile(paths.claude, "utf8")).toBe("Imported drift\n");
			expect(result.overwritten.snapshot.content).toBe("Canonical\n");
			expect(result.imported.snapshot.content).toBe("Imported drift\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects drift actions that were never recorded in the durable projection", async () => {
		const paths = await make_paths();
		const actions = ["ignore", "import", "overwrite"] as const;

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() =>
						writeFile(paths.claude, "Unobserved drift\n", "utf8"),
					);

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejections = yield* Effect.forEach(actions, (action) =>
						guidance
							.ResolveDrift({
								...trace(`guidance_unobserved_${action}`),
								action,
								observed_hash: guidance_hash("Unobserved drift\n"),
								provider: "claude",
							})
							.pipe(
								Effect.match({
									onFailure: (error) => error,
									onSuccess: () => "unexpected_success" as const,
								}),
							),
					);

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						rejections,
					};
				}),
			);

			expect(result.rejections).toHaveLength(actions.length);
			expect(result.rejections).toEqual(
				expect.arrayContaining(
					actions.map(() =>
						expect.objectContaining({
							_tag: "GlobalGuidanceConflict",
							reason: "drift_changed",
						}),
					),
				),
			);
			expect(result.after).toEqual(result.before);
			expect(await readFile(paths.canonical, "utf8")).toBe("Initial\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Unobserved drift\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("records a partial sync failure and retries after the path becomes writable", async () => {
		const paths = await make_paths();
		const blocked_parent = join(paths.root, "claude");

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Chosen\n", "utf8");
		await writeFile(blocked_parent, "not a directory", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;
					const initial = yield* guidance.Initialize;

					yield* Effect.promise(() => rm(blocked_parent, { force: true }));
					yield* Effect.promise(() => mkdir(blocked_parent, { recursive: true }));
					const retried = yield* guidance.RetrySync({
						...trace("guidance_retry_claude"),
						provider: "claude",
					});

					return { initial, retried };
				}),
			);

			expect(result.initial.metadata.canonical.status).toBe("ready");
			expect(
				result.initial.metadata.providers.find(
					(provider) => provider.provider === "claude",
				),
			).toMatchObject({ status: "sync_failed" });
			expect(
				result.retried.snapshot.metadata.providers.find(
					(provider) => provider.provider === "claude",
				),
			).toMatchObject({ status: "synced" });
			expect(await readFile(paths.claude, "utf8")).toBe("Chosen\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an accepted sync retry after canonical guidance changes", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Canonical A\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					const accepted = yield* guidance.RetrySync({
						...trace("guidance_retry_replay"),
						provider: "codex",
					});
					yield* guidance.Update({
						...trace("guidance_retry_new_canonical"),
						content: "Canonical B\n",
					});
					yield* Effect.promise(() =>
						writeFile(paths.codex_agents, "External after retry\n", "utf8"),
					);

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const duplicate = yield* guidance.RetrySync({
						...trace("guidance_retry_replay"),
						provider: "codex",
						sent_at: "2026-07-11T12:01:00.000Z",
					});

					return {
						accepted,
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						duplicate,
						provider: yield* Effect.promise(() => readFile(paths.codex_agents, "utf8")),
					};
				}),
			);

			expect(result.accepted.acceptance.status).toBe("accepted");
			expect(result.duplicate.acceptance.status).toBe("duplicate");
			expect(result.after).toEqual(result.before);
			expect(result.provider).toBe("External after retry\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("preflights provider overwrite retries before touching a newly drifted mirror", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* guidance.Update({
						...trace("guidance_provider_canonical"),
						content: "Canonical\n",
					});
					yield* Effect.promise(() => writeFile(paths.claude, "Drift A\n", "utf8"));
					yield* guidance.Get;
					const observed_hash = guidance_hash("Drift A\n");
					const accepted = yield* guidance.ResolveDrift({
						...trace("guidance_provider_overwrite"),
						action: "overwrite",
						observed_hash,
						provider: "claude",
					});

					yield* Effect.promise(() => writeFile(paths.claude, "Drift B\n", "utf8"));
					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const duplicate = yield* guidance.ResolveDrift({
						...trace("guidance_provider_overwrite"),
						action: "overwrite",
						observed_hash,
						provider: "claude",
						sent_at: "2026-07-11T12:01:00.000Z",
					});
					const conflict = yield* guidance
						.ResolveDrift({
							...trace("guidance_provider_overwrite"),
							action: "overwrite",
							observed_hash: guidance_hash("Drift B\n"),
							provider: "claude",
						})
						.pipe(Effect.exit);

					return {
						accepted,
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						conflict,
						duplicate,
						provider_content: yield* Effect.promise(() =>
							readFile(paths.claude, "utf8"),
						),
					};
				}),
			);

			expect(result.accepted.acceptance.status).toBe("accepted");
			expect(result.duplicate.acceptance.status).toBe("duplicate");
			expect(result.conflict._tag).toBe("Failure");
			expect(result.provider_content).toBe("Drift B\n");
			expect(result.after).toEqual(result.before);
		} finally {
			await runtime.dispose();
		}
	});

	it("replays an accepted drift import before consulting changed provider state", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() => writeFile(paths.claude, "Drift A\n", "utf8"));
					yield* guidance.Get;

					const observed_hash = guidance_hash("Drift A\n");
					const accepted = yield* guidance.ResolveDrift({
						...trace("guidance_import_replay"),
						action: "import",
						observed_hash,
						provider: "claude",
					});

					yield* Effect.promise(() => writeFile(paths.claude, "Drift B\n", "utf8"));

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const duplicate = yield* guidance.ResolveDrift({
						...trace("guidance_import_replay"),
						action: "import",
						observed_hash,
						provider: "claude",
						sent_at: "2026-07-11T12:01:00.000Z",
					});

					return {
						accepted,
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						duplicate,
						provider: yield* Effect.promise(() => readFile(paths.claude, "utf8")),
					};
				}),
			);

			expect(result.accepted.acceptance.status).toBe("accepted");
			expect(result.duplicate.acceptance.status).toBe("duplicate");
			expect(result.after).toEqual(result.before);
			expect(result.canonical).toBe("Drift A\n");
			expect(result.provider).toBe("Drift B\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("records drift when a provider changes immediately after write verification", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.claude, "Initial\n", "utf8");
		const mutation = make_mutating_claude_registry(
			paths.claude,
			"External after write\n",
			(_discovery_count, current_content) => current_content === "Replacement\n",
		);
		const runtime = make_runtime(paths, { registry: mutation.registry });

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.sync(mutation.arm);

					return (yield* guidance.Update({
						...trace("guidance_post_write_drift"),
						content: "Replacement\n",
					})).snapshot;
				}),
			);
			const claude = snapshot.metadata.providers.find(
				(provider) => provider.provider === "claude",
			)!;

			expect(snapshot.content).toBe("Replacement\n");
			expect(claude).toMatchObject({
				applied_hash: guidance_hash("Replacement\n"),
				observed_hash: guidance_hash("External after write\n"),
				status: "drift_detected",
			});
			expect(await readFile(paths.claude, "utf8")).toBe("External after write\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("fences a provider overwrite when drift changes between discovery and write", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.claude, "Initial\n", "utf8");
		const mutation = make_mutating_claude_registry(
			paths.claude,
			"Drift B\n",
			(discovery_count) => discovery_count === 2,
		);
		const runtime = make_runtime(paths, { registry: mutation.registry });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() => writeFile(paths.claude, "Drift A\n", "utf8"));
					yield* guidance.Get;

					const before = {
						backups: yield* Effect.promise(() =>
							readdir(paths.backups).catch(() => []),
						),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};

					yield* Effect.sync(mutation.arm);
					const rejection = yield* guidance
						.ResolveDrift({
							...trace("guidance_overwrite_toctou"),
							action: "overwrite",
							observed_hash: guidance_hash("Drift A\n"),
							provider: "claude",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);

					return {
						after: {
							backups: yield* Effect.promise(() =>
								readdir(paths.backups).catch(() => []),
							),
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						before,
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						provider: yield* Effect.promise(() => readFile(paths.claude, "utf8")),
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceConflict",
				reason: "drift_changed",
			});
			expect(result.after).toEqual(result.before);
			expect(result.canonical).toBe("Initial\n");
			expect(result.provider).toBe("Drift B\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves a provider edit that lands after the overwrite hash fence", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths, {
			mutate_before_replace: {
				path: paths.claude,
				replacement: "Drift B\n",
			},
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() => writeFile(paths.claude, "Drift A\n", "utf8"));
					yield* guidance.Get;

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejection = yield* guidance
						.ResolveDrift({
							...trace("guidance_overwrite_post_fence_race"),
							action: "overwrite",
							observed_hash: guidance_hash("Drift A\n"),
							provider: "claude",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);
					const backup_contents = yield* Effect.promise(async () => {
						const entries = await readdir(paths.backups);

						return Promise.all(
							entries.map((entry) => readFile(join(paths.backups, entry), "utf8")),
						);
					});

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						backup_contents,
						before,
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						provider: yield* Effect.promise(() => readFile(paths.claude, "utf8")),
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceConflict",
				reason: "drift_changed",
			});
			expect(result.after).toEqual(result.before);
			expect(result.backup_contents).toContain("Drift B\n");
			expect(result.canonical).toBe("Initial\n");
			expect(result.provider).toBe("Drift B\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("retains backup evidence when a provider write fails after backup", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Initial\n", "utf8");
		await writeFile(paths.claude, "Initial\n", "utf8");
		const runtime = make_runtime(paths, { fail_provider_write: paths.claude });

		try {
			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;

					return (yield* guidance.Update({
						...trace("guidance_backup_failure"),
						content: "Replacement\n",
					})).snapshot;
				}),
			);
			const claude = snapshot.metadata.providers.find(
				(provider) => provider.provider === "claude",
			)!;

			expect(claude).toMatchObject({
				backup_path: expect.any(String),
				last_error_code: "guidance_replace_failed",
				status: "sync_failed",
			});
			expect(await readFile(claude.backup_path!, "utf8")).toBe("Initial\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Initial\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("preserves an external canonical edit that lands after the update hash fence", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Canonical A\n", "utf8");
		const runtime = make_runtime(paths, {
			mutate_before_replace: {
				path: paths.canonical,
				replacement: "External canonical C\n",
				when_content: "User canonical B\n",
			},
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;

					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const rejection = yield* guidance
						.Update({
							...trace("guidance_canonical_post_fence_race"),
							content: "User canonical B\n",
						})
						.pipe(
							Effect.match({
								onFailure: (error) => error,
								onSuccess: () => "unexpected_success" as const,
							}),
						);
					const backup_contents = yield* Effect.promise(async () => {
						const entries = await readdir(paths.backups);

						return Promise.all(
							entries.map((entry) => readFile(join(paths.backups, entry), "utf8")),
						);
					});

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
						backup_contents,
						before,
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						provider: yield* Effect.promise(() => readFile(paths.codex_agents, "utf8")),
						rejection,
					};
				}),
			);

			expect(result.rejection).toMatchObject({
				_tag: "GlobalGuidanceInvariantError",
				operation: "canonical_changed_during_write",
			});
			expect(result.after).toEqual(result.before);
			expect(result.backup_contents).toContain("External canonical C\n");
			expect(result.canonical).toBe("External canonical C\n");
			expect(result.provider).toBe("Canonical A\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("preflights canonical replays and conflicts before changing files or metadata", async () => {
		const paths = await make_paths();
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* guidance.Update({ ...trace("guidance_a"), content: "A\n" });
					yield* guidance.Update({ ...trace("guidance_b"), content: "B\n" });
					const before = {
						canonical: yield* Effect.promise(() => readFile(paths.canonical, "utf8")),
						claude: yield* Effect.promise(() => readFile(paths.claude, "utf8")),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					};
					const replay = yield* guidance.Update({
						...trace("guidance_a"),
						content: "A\n",
						sent_at: "2026-07-11T12:01:00.000Z",
					});
					const conflict = yield* guidance
						.Update({ ...trace("guidance_a"), content: "conflict\n" })
						.pipe(Effect.exit);

					return {
						...before,
						conflict,
						replay,
						after: {
							canonical: yield* Effect.promise(() =>
								readFile(paths.canonical, "utf8"),
							),
							claude: yield* Effect.promise(() => readFile(paths.claude, "utf8")),
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
							providers: yield* database.client
								.select()
								.from(GlobalGuidanceProviderSync),
						},
					};
				}),
			);

			expect(result.replay.acceptance.status).toBe("duplicate");
			expect(result.conflict._tag).toBe("Failure");
			expect(result.after).toEqual({
				canonical: result.canonical,
				claude: result.claude,
				commands: result.commands,
				events: result.events,
				providers: result.providers,
			});
			expect(result.after.canonical).toBe("B\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps unresolved first-run selection stable across initialize and get", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await mkdir(join(paths.root, "claude"), { recursive: true });
		await writeFile(paths.codex_agents, "Codex\n", "utf8");
		await writeFile(paths.claude, "Claude\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;
					const initial = yield* guidance.Initialize;
					const before = {
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
					const repeated = yield* guidance.Initialize;
					const fetched = yield* guidance.Get;

					return {
						after: {
							commands: yield* database.client.select().from(JournalCommands),
							events: yield* database.client.select().from(JournalEvents),
						},
						before,
						fetched,
						initial,
						repeated,
					};
				}),
			);

			expect(result.repeated.candidates).toEqual(result.initial.candidates);
			expect(result.fetched.candidates).toEqual(result.initial.candidates);
			expect(result.after).toEqual(result.before);
		} finally {
			await runtime.dispose();
		}
	});

	it("restores a missing canonical file only from a verified matching provider", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Verified\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const restored = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() => rm(paths.canonical));

					return yield* guidance.Get;
				}),
			);

			expect(restored.content).toBe("Verified\n");
			expect(await readFile(paths.canonical, "utf8")).toBe("Verified\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("does not repair a missing canonical file from a mismatched provider", async () => {
		const paths = await make_paths();

		await mkdir(join(paths.root, "codex"), { recursive: true });
		await writeFile(paths.codex_agents, "Original\n", "utf8");
		const runtime = make_runtime(paths);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* Effect.promise(() => rm(paths.canonical));
					yield* Effect.promise(() => writeFile(paths.codex_agents, "Changed\n", "utf8"));
					yield* Effect.promise(() => writeFile(paths.claude, "Changed\n", "utf8"));
					const failure = yield* guidance.Get.pipe(
						Effect.match({
							onFailure: (error) => error._tag,
							onSuccess: () => "unexpected_success",
						}),
					);

					return {
						failure,
						provider: yield* Effect.promise(() => readFile(paths.codex_agents, "utf8")),
					};
				}),
			);

			expect(result.failure).toBe("GlobalGuidanceInvariantError");
			expect(result.provider).toBe("Changed\n");
			expect(await readFile(paths.claude, "utf8")).toBe("Changed\n");
		} finally {
			await runtime.dispose();
		}
	});

	it("never persists user guidance content in SQLite commands, events, or projections", async () => {
		const paths = await make_paths();
		const secret = "private global guidance that must remain file-only";
		const runtime = make_runtime(paths);

		try {
			const persisted = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const guidance = yield* GlobalGuidanceService;

					yield* guidance.Initialize;
					yield* guidance.Update({
						...trace("guidance_secret_update"),
						content: secret,
					});

					return JSON.stringify({
						canonical: yield* database.client.select().from(GlobalGuidanceCanonical),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
						providers: yield* database.client.select().from(GlobalGuidanceProviderSync),
					});
				}),
			);

			expect(persisted).not.toContain(secret);
			expect(await readFile(paths.canonical, "utf8")).toBe(`${secret}\n`);
		} finally {
			await runtime.dispose();
		}
	});
});
