import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Fiber, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ModelBehaviourProviderState } from "@artisan/protocol";

import {
	ModelBehaviourConfigFiles,
	make_model_behaviour_config_files_layer,
} from "../../modules/backend/src/model-behaviour/config-files";
import {
	make_codex_model_behaviour_provider,
	make_inactive_model_behaviour_provider,
	make_model_behaviour_provider_registry_layer,
	ModelBehaviourProviderRegistry,
	ModelBehaviourProviderRegistryError,
	type ModelBehaviourProviderAdapter,
} from "../../modules/backend/src/model-behaviour/provider";
import {
	make_codex_auto_compaction_mapping,
	make_unsupported_auto_compaction_mapping,
} from "../../modules/backend/src/model-behaviour/registry";
import {
	ModelBehaviourService,
	ModelBehaviourServiceLive,
	make_model_behaviour_service_layer,
	type ModelBehaviourServiceOptions,
} from "../../modules/backend/src/model-behaviour/service";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreFailure } from "../../modules/backend/src/persistence/journal-store";
import { JournalCommands, JournalEvents } from "../../modules/backend/src/persistence/tables";
import {
	ModelBehaviourRepository,
	ModelBehaviourRepositoryLive,
} from "../../modules/backend/src/model-behaviour/repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(`${tmpdir()}/artisan model behaviour service `);

	roots.push(root);

	return root;
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "model_behaviour_service_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T16:30:00.000Z"),
	});
}

async function make_harness(
	options: {
		readonly codex_supported?: boolean;
		readonly inspect?: (
			Inspect: ModelBehaviourProviderAdapter["Inspect"],
		) => ModelBehaviourProviderAdapter["Inspect"];
		readonly fail_first_commit?: boolean;
		readonly on_read?: () => void;
		readonly registry?: Layer.Layer<ModelBehaviourProviderRegistry>;
		readonly root?: string;
		readonly service_options?: ModelBehaviourServiceOptions;
	} = {},
) {
	const root = options.root ?? (await make_root());
	const config_path = join(root, "codex home", "config.toml");
	const files = await Effect.runPromise(
		Effect.service(ModelBehaviourConfigFiles).pipe(
			Effect.provide(make_model_behaviour_config_files_layer()),
		),
	);
	const codex_mapping = make_codex_auto_compaction_mapping({
		installed_version: "0.142.5",
		mapping_available: options.codex_supported ?? true,
	});
	const codex_base =
		codex_mapping.state === "supported"
			? make_codex_model_behaviour_provider({
					backups_directory: join(root, "backups"),
					files,
					mapping: codex_mapping,
					target_path: config_path,
				})
			: make_inactive_model_behaviour_provider(codex_mapping);
	const codex =
		options.inspect === undefined
			? codex_base
			: { ...codex_base, Inspect: options.inspect(codex_base.Inspect) };
	const claude = make_inactive_model_behaviour_provider(
		make_unsupported_auto_compaction_mapping(
			"claude",
			"Claude Code has no equivalent supported mapping.",
		),
	);
	const metadata = make_metadata_layer();
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path: join(root, "artisan.db"), migrations_path }),
		metadata,
		JournalNotifierLive,
	);
	const live_repository = ModelBehaviourRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const failure_repository =
		options.fail_first_commit === true
			? Layer.effect(
					ModelBehaviourRepository,
					Effect.gen(function* () {
						const live = yield* ModelBehaviourRepository;
						let failed = false;

						return {
							...live,
							Commit: (input) =>
								Effect.suspend(() => {
									if (!failed) {
										failed = true;

										return Effect.fail(
											new JournalStoreFailure({
												cause: new Error(
													"injected post-provider commit failure",
												),
											}),
										);
									}

									return live.Commit(input);
								}),
						};
					}),
				).pipe(Layer.provideMerge(live_repository))
			: live_repository;
	const on_read = options.on_read;
	const repository =
		on_read === undefined
			? failure_repository
			: Layer.effect(
					ModelBehaviourRepository,
					Effect.gen(function* () {
						const live = yield* ModelBehaviourRepository;
						return {
							...live,
							Read: Effect.sync(on_read).pipe(Effect.andThen(live.Read)),
						};
					}),
				).pipe(Layer.provideMerge(failure_repository));
	const registry =
		options.registry ?? make_model_behaviour_provider_registry_layer([codex, claude]);
	const service = (
		options.service_options === undefined
			? ModelBehaviourServiceLive
			: make_model_behaviour_service_layer(options.service_options)
	).pipe(
		Layer.provideMerge(repository),
		Layer.provideMerge(registry),
		Layer.provideMerge(metadata),
	);

	return {
		config_path,
		root,
		runtime: ManagedRuntime.make(service),
	};
}

type Harness = Awaited<ReturnType<typeof make_harness>>;

function update_input(message_id: string, value: number) {
	return {
		message_id,
		origin: "frontend" as const,
		sent_at: "2026-07-11T16:30:00.000Z",
		setting_id: "auto_compaction_trigger_tokens" as const,
		value: { type: "integer" as const, value },
	};
}

function provider(
	snapshot: { readonly providers: ReadonlyArray<ModelBehaviourProviderState> },
	provider_id: string,
) {
	return snapshot.providers.find((state) => state.provider_id === provider_id)!;
}

async function get_snapshot(harness: Harness) {
	return harness.runtime.runPromise(
		Effect.gen(function* () {
			const service = yield* ModelBehaviourService;

			return yield* service.Get;
		}),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("ModelBehaviourService", () => {
	it("projects provider support and observes absent Codex config without creating it", async () => {
		const harness = await make_harness();

		try {
			const snapshot = await get_snapshot(harness);
			const events = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return yield* database.client.select().from(JournalEvents);
				}),
			);

			expect(snapshot.capabilities[0]!.provider_support).toMatchObject([
				{ provider_id: "codex", state: "supported" },
				{ provider_id: "claude", state: "unsupported" },
			]);
			expect(provider(snapshot, "codex").status).toBe("provider_default");
			expect(provider(snapshot, "claude").status).toBe("unsupported");
			expect(events.map((event) => event.event_type)).toEqual([
				"model_behaviour.provider.reconciled",
				"model_behaviour.provider.reconciled",
			]);
			await expect(fs.access(harness.config_path)).rejects.toBeDefined();
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("updates Codex exactly once and rejects changed intent before another write", async () => {
		const harness = await make_harness();
		const original = '# keep\r\napi_key = "secret"\r\n';

		try {
			await fs.mkdir(join(harness.root, "codex home"), { recursive: true });
			await fs.writeFile(harness.config_path, original, "utf8");

			const result = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;
					const accepted = yield* service.Update(update_input("update_exact", 250_000));
					const duplicate = yield* service.Update(update_input("update_exact", 250_000));
					const conflict = yield* service
						.Update(update_input("update_exact", 300_000))
						.pipe(Effect.exit);

					return { accepted, conflict, duplicate };
				}),
			);
			const content = await fs.readFile(harness.config_path, "utf8");
			const backups = await fs.readdir(join(harness.root, "backups"));
			const backup_files = backups.filter(
				(name) => name.includes(".original-") && !name.endsWith(".permissions.json"),
			);
			const permission_files = backups.filter((name) => name.endsWith(".permissions.json"));
			const publication_anchors = backups.filter((name) => name.includes(".replacement-"));

			expect(result.accepted.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.duplicate.events).toEqual(result.accepted.events);
			expect(result.conflict._tag).toBe("Failure");
			expect(JSON.stringify(result.conflict)).toContain("CommandIdConflict");
			expect(content).toContain('api_key = "secret"');
			expect(content).toContain("model_auto_compact_token_limit = 250000");
			expect(backup_files).toHaveLength(1);
			expect(permission_files).toHaveLength(1);
			expect(publication_anchors).toHaveLength(1);
			expect(result.accepted.snapshot.settings[0]).toMatchObject({
				value: { type: "integer", value: 250_000 },
				version: 1,
			});
			expect(provider(result.accepted.snapshot, "codex").status).toBe("synced");
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("disables updates when no installed provider supports the control", async () => {
		const harness = await make_harness({ codex_supported: false });

		try {
			const result = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service
						.Update(update_input("unsupported_update", 250_000))
						.pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).toContain("no_supported_provider");
			await expect(fs.access(harness.config_path)).rejects.toBeDefined();
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("detects, ignores, reopens, imports, and overwrites exact provider drift", async () => {
		const harness = await make_harness();

		try {
			await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					yield* service.Update(update_input("drift_seed", 250_000));
				}),
			);
			await fs.writeFile(
				harness.config_path,
				"model_auto_compact_token_limit = 300000\nexternal = true\n",
				"utf8",
			);

			const drift = await get_snapshot(harness);
			const drift_events = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return (yield* database.client.select().from(JournalEvents)).length;
				}),
			);
			const stable = await get_snapshot(harness);
			const stable_events = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return (yield* database.client.select().from(JournalEvents)).length;
				}),
			);
			const observed_hash = provider(drift, "codex").observed_hash!;
			const ignored = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service.ResolveDrift({
						action: "ignore",
						message_id: "drift_ignore",
						observed_hash,
						origin: "frontend",
						provider_id: "codex",
						sent_at: "2026-07-11T16:30:00.000Z",
						setting_id: "auto_compaction_trigger_tokens",
					});
				}),
			);

			expect(provider(drift, "codex").status).toBe("drift_detected");
			expect(provider(stable, "codex").status).toBe("drift_detected");
			expect(stable_events).toBe(drift_events);
			expect(provider(ignored.snapshot, "codex").status).toBe("drift_ignored");

			await fs.writeFile(
				harness.config_path,
				"model_auto_compact_token_limit = 350000\nexternal = true\n",
				"utf8",
			);
			const reopened = await get_snapshot(harness);
			const imported = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service.ResolveDrift({
						action: "import",
						message_id: "drift_import",
						observed_hash: provider(reopened, "codex").observed_hash!,
						origin: "frontend",
						provider_id: "codex",
						sent_at: "2026-07-11T16:30:00.000Z",
						setting_id: "auto_compaction_trigger_tokens",
					});
				}),
			);

			expect(provider(reopened, "codex").status).toBe("drift_detected");
			expect(imported.snapshot.settings[0]!.value).toEqual({
				type: "integer",
				value: 350_000,
			});
			expect(provider(imported.snapshot, "codex").status).toBe("synced");

			await fs.writeFile(
				harness.config_path,
				"model_auto_compact_token_limit = 400000\nexternal = true\n",
				"utf8",
			);
			const overwrite_drift = await get_snapshot(harness);
			const overwritten = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service.ResolveDrift({
						action: "overwrite",
						message_id: "drift_overwrite",
						observed_hash: provider(overwrite_drift, "codex").observed_hash!,
						origin: "frontend",
						provider_id: "codex",
						sent_at: "2026-07-11T16:30:00.000Z",
						setting_id: "auto_compaction_trigger_tokens",
					});
				}),
			);

			expect(provider(overwritten.snapshot, "codex").status).toBe("synced");
			expect(await fs.readFile(harness.config_path, "utf8")).toContain(
				"model_auto_compact_token_limit = 350000",
			);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("commits canonical intent on provider failure and supports an explicit retry", async () => {
		const harness = await make_harness();
		const malformed = "broken = [\n";

		try {
			await fs.mkdir(join(harness.root, "codex home"), { recursive: true });
			await fs.writeFile(harness.config_path, malformed, "utf8");

			const failed = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service.Update(update_input("failed_sync", 250_000));
				}),
			);

			expect(failed.snapshot.settings[0]!.value).toEqual({
				type: "integer",
				value: 250_000,
			});
			expect(provider(failed.snapshot, "codex").status).toBe("sync_failed");
			expect(await fs.readFile(harness.config_path, "utf8")).toBe(malformed);

			await fs.writeFile(harness.config_path, "# repaired\n", "utf8");
			const retried = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service.RetrySync({
						message_id: "retry_sync",
						origin: "frontend",
						provider_id: "codex",
						sent_at: "2026-07-11T16:30:00.000Z",
						setting_id: "auto_compaction_trigger_tokens",
					});
				}),
			);

			expect(provider(retried.snapshot, "codex").status).toBe("synced");
			expect(await fs.readFile(harness.config_path, "utf8")).toContain(
				"model_auto_compact_token_limit = 250000",
			);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("rejects a stale drift action without changing the newer provider file", async () => {
		const harness = await make_harness();

		try {
			await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					yield* service.Update(update_input("stale_seed", 250_000));
				}),
			);
			await fs.writeFile(
				harness.config_path,
				"model_auto_compact_token_limit = 300000\n",
				"utf8",
			);
			const first_drift = await get_snapshot(harness);
			const stale_hash = provider(first_drift, "codex").observed_hash!;
			const newer = "model_auto_compact_token_limit = 350000\nnewer = true\n";

			await fs.writeFile(harness.config_path, newer, "utf8");

			const result = await harness.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service
						.ResolveDrift({
							action: "overwrite",
							message_id: "stale_overwrite",
							observed_hash: stale_hash,
							origin: "frontend",
							provider_id: "codex",
							sent_at: "2026-07-11T16:30:00.000Z",
							setting_id: "auto_compaction_trigger_tokens",
						})
						.pipe(Effect.exit);
				}),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).toContain("stale_observation");
			expect(await fs.readFile(harness.config_path, "utf8")).toBe(newer);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("recovers an exact retry after the provider write succeeds but SQLite commit fails", async () => {
		const first = await make_harness({ fail_first_commit: true });
		const original = '# original\napi_key = "preserved"\n';

		await fs.mkdir(join(first.root, "codex home"), { recursive: true });
		await fs.writeFile(first.config_path, original, "utf8");

		try {
			const failed = await first.runtime.runPromise(
				Effect.gen(function* () {
					const service = yield* ModelBehaviourService;

					return yield* service
						.Update(update_input("crash_retry", 250_000))
						.pipe(Effect.exit);
				}),
			);

			expect(failed._tag).toBe("Failure");
			expect(await fs.readFile(first.config_path, "utf8")).toContain(
				"model_auto_compact_token_limit = 250000",
			);
			const backup_entries = await fs.readdir(join(first.root, "backups"));

			expect(
				backup_entries.filter(
					(name) => name.includes(".original-") && !name.endsWith(".permissions.json"),
				),
			).toHaveLength(1);
			expect(
				backup_entries.filter((name) => name.endsWith(".permissions.json")),
			).toHaveLength(1);
			expect(backup_entries.filter((name) => name.includes(".replacement-"))).toHaveLength(1);
		} finally {
			await first.runtime.dispose();
		}

		const restarted = await make_harness({ root: first.root });

		try {
			const result = await restarted.runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ModelBehaviourService;
					const accepted = yield* service.Update(update_input("crash_retry", 250_000));

					return {
						accepted,
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.commands).toHaveLength(1);
			expect(result.events).toHaveLength(3);
			const final_backup_entries = await fs.readdir(join(first.root, "backups"));

			expect(
				final_backup_entries.filter(
					(name) => name.includes(".original-") && !name.endsWith(".permissions.json"),
				),
			).toHaveLength(1);
			expect(
				final_backup_entries.filter((name) => name.endsWith(".permissions.json")),
			).toHaveLength(1);
			expect(
				final_backup_entries.filter((name) => name.includes(".replacement-")),
			).toHaveLength(1);
			expect(await fs.readFile(first.config_path, "utf8")).toContain(
				"model_auto_compact_token_limit = 250000",
			);
		} finally {
			await restarted.runtime.dispose();
		}
	});

	it("coalesces overlapping refreshes behind a service-owned flight and refreshes a later Get", async () => {
		const entered = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let inspections = 0;
		let reads = 0;
		const harness = await make_harness({
			inspect: (Inspect) =>
				Effect.sync(() => {
					inspections += 1;
				}).pipe(
					Effect.andThen(Deferred.succeed(entered, undefined)),
					Effect.andThen(Deferred.await(release)),
					Effect.andThen(Inspect),
				),
			on_read: () => {
				reads += 1;
			},
		});

		try {
			const service = await harness.runtime.runPromise(Effect.service(ModelBehaviourService));
			const interrupted = Effect.runFork(service.Get);
			await Effect.runPromise(Deferred.await(entered));
			await Effect.runPromise(Fiber.interrupt(interrupted));
			const callers = Array.from({ length: 6 }, () => Effect.runPromise(service.Get));

			expect(inspections).toBe(1);
			await Effect.runPromise(Deferred.succeed(release, undefined));
			const snapshots = await Promise.all(callers);
			expect(snapshots.every((snapshot) => snapshot === snapshots[0])).toBe(true);
			expect(inspections).toBe(1);
			expect(reads).toBe(2);

			await Effect.runPromise(service.Get);
			expect(inspections).toBe(2);
			expect(reads).toBe(4);
		} finally {
			await harness.runtime.dispose();
		}
	});

	it("settles a claimed refresh on close before scope admission without late provider work", async () => {
		const claimed = await Effect.runPromise(Deferred.make<void>());
		const release_claim = await Effect.runPromise(Deferred.make<void>());
		let inspections = 0;
		let reads = 0;
		const harness = await make_harness({
			inspect: (Inspect) =>
				Effect.sync(() => {
					inspections += 1;
				}).pipe(Effect.andThen(Inspect)),
			on_read: () => {
				reads += 1;
			},
			service_options: {
				OnRefreshClaimed: Deferred.succeed(claimed, undefined).pipe(
					Effect.andThen(Deferred.await(release_claim)),
				),
			},
		});
		const service = await harness.runtime.runPromise(Effect.service(ModelBehaviourService));
		const waiter = Effect.runPromiseExit(service.Get);

		await Effect.runPromise(Deferred.await(claimed));
		await harness.runtime.dispose();
		await Effect.runPromise(Deferred.succeed(release_claim, undefined));
		expect((await waiter)._tag).toBe("Failure");
		expect(inspections).toBe(0);
		expect(reads).toBe(0);
	});

	it("settles external waiters when close interrupts completion publication", async () => {
		const publishing = await Effect.runPromise(Deferred.make<void>());
		const never_release = await Effect.runPromise(Deferred.make<void>());
		const harness = await make_harness({
			service_options: {
				OnRefreshPublishing: Deferred.succeed(publishing, undefined).pipe(
					Effect.andThen(Deferred.await(never_release)),
				),
			},
		});
		const service = await harness.runtime.runPromise(Effect.service(ModelBehaviourService));
		const waiter = Effect.runPromiseExit(service.Get);

		await Effect.runPromise(Deferred.await(publishing));
		await harness.runtime.dispose();
		expect((await waiter)._tag).toBe("Failure");
	});

	it("maps typed registry discovery failure to its renderer-safe invariant", async () => {
		const registry_error = new ModelBehaviourProviderRegistryError({
			cause: new Error("private discovery detail"),
		});
		const harness = await make_harness({
			registry: Layer.succeed(ModelBehaviourProviderRegistry, {
				Await: Effect.fail(registry_error),
			}),
		});

		try {
			const result = await harness.runtime.runPromise(
				Effect.service(ModelBehaviourService).pipe(
					Effect.flatMap((service) => service.Get),
					Effect.exit,
				),
			);

			expect(result._tag).toBe("Failure");
			expect(JSON.stringify(result)).toContain("ModelBehaviourInvariantError");
			expect(JSON.stringify(result)).not.toContain("private discovery detail");
		} finally {
			await harness.runtime.dispose();
		}
	});
});
