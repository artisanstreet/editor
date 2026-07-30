import { createHash } from "node:crypto";
import {
	access,
	mkdtemp,
	mkdir,
	readFile,
	readdir,
	rm,
	symlink,
	writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { EngineProcessFactory } from "@artisan/engines";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_local_routine_installer_layer,
	make_local_routine_source_inspector_layer,
	make_npx_skills_process_adapter_layer,
} from "../../modules/backend/src/marketplace/routines/production-adapters";
import {
	NpxSkillsAdapter,
	RoutineInstaller,
	RoutineSourceInspector,
} from "../../modules/backend/src/marketplace/routines/adapters";

const roots: Array<string> = [];
const TemporaryRoot = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-routine-adapter-"));
	roots.push(root);
	return root;
};

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe("production Routine adapters", () => {
	it("inspects bounded local skills deterministically without writing", async () => {
		const root = await TemporaryRoot();
		await writeFile(
			join(root, "SKILL.md"),
			"---\nname: Local Helper\ndescription: Safe local routine\nversion: 1.2.3\n---\n\nDo the work.\n",
			"utf8",
		);
		await mkdir(join(root, "references"));
		await writeFile(join(root, "references", "notes.md"), "Evidence", "utf8");
		await writeFile(
			join(root, "artisan-routine.json"),
			JSON.stringify({
				exported_commands: [{ description: "Run safely", name: "run" }],
				permissions: [
					{ description: "Read selected project files", kind: "filesystem_read" },
				],
			}),
			"utf8",
		);
		const layer = make_local_routine_source_inspector_layer();
		const Inspect = () =>
			Effect.runPromise(
				Effect.gen(function* () {
					return yield* (yield* RoutineSourceInspector).Inspect({
						scope: { kind: "global" },
						source: { kind: "local", locator: root },
					});
				}).pipe(Effect.provide(layer)),
			);
		const first = await Inspect();
		const second = await Inspect();
		expect(first).toEqual(second);
		expect(first).toMatchObject({
			display_name: "Local Helper",
			trust: "local",
			version: "1.2.3",
		});
		expect(first.files.map((file) => file.path)).toEqual([
			"SKILL.md",
			"artisan-routine.json",
			"references/notes.md",
		]);
	});

	it("rejects symlinks and file-count/output bounds", async () => {
		const root = await TemporaryRoot();
		const outside = await TemporaryRoot();
		await writeFile(join(root, "SKILL.md"), "Instructions", "utf8");
		await writeFile(join(outside, "secret.txt"), "secret", "utf8");
		try {
			await symlink(join(outside, "secret.txt"), join(root, "escape.txt"));
		} catch (cause) {
			/** Windows hosts without Developer Mode cannot create a symlink; retain bound coverage. */
			if (!(cause instanceof Error && "code" in cause && cause.code === "EPERM")) throw cause;
			await writeFile(join(root, "one.txt"), "one", "utf8");
			await writeFile(join(root, "two.txt"), "two", "utf8");
		}
		const rejected = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* RoutineSourceInspector).Inspect({
					scope: { kind: "global" },
					source: { kind: "local", locator: root },
				});
			}).pipe(
				Effect.exit,
				Effect.provide(make_local_routine_source_inspector_layer({ max_files: 2 })),
			),
		);
		expect(rejected._tag).toBe("Failure");
	});

	it("atomically materializes, retries exactly, cleans stale stages and rolls back idempotently", async () => {
		const source = await TemporaryRoot();
		const install_root = join(await TemporaryRoot(), "installed");
		const content = new TextEncoder().encode("Local instructions");
		await writeFile(join(source, "SKILL.md"), content);
		await mkdir(install_root, { recursive: true });
		await mkdir(
			join(
				install_root,
				`.stage-${createHash("sha256").update("operation").digest("hex").slice(0, 32)}`,
			),
		);
		const receipt = await Effect.runPromise(
			Effect.gen(function* () {
				const installer = yield* RoutineInstaller;
				const inspection = {
					artifact_refs: [
						`sha256:${createHash("sha256").update(content).digest("hex")}:SKILL.md`,
					],
					candidate_id: "routine_local",
					compatibility: [],
					content_hashes: {
						"SKILL.md": createHash("sha256").update(content).digest("hex"),
					},
					description: "Local",
					display_name: "Local",
					exported_commands: [],
					files: [{ path: "SKILL.md", required: true }],
					instructions: "Local",
					permissions: [],
					rollback_available: true,
					source: { kind: "local" as const, locator: source },
					trust: "local" as const,
					version: "1",
				};
				const first = yield* installer.Install({
					inspection,
					operation_id: "operation",
					scope: { kind: "global" },
				});
				const second = yield* installer.Install({
					inspection,
					operation_id: "operation",
					scope: { kind: "global" },
				});
				yield* Effect.promise(() => writeFile(join(source, "SKILL.md"), "changed"));
				const changed_retry = yield* Effect.exit(
					installer.Install({
						inspection,
						operation_id: "operation",
						scope: { kind: "global" },
					}),
				);
				const installed_before_rollback = yield* Effect.promise(async () => {
					const [target] = (await readdir(install_root)).filter(
						(entry) => !entry.startsWith("."),
					);
					return new TextDecoder().decode(
						await readFile(join(install_root, target!, "SKILL.md")),
					);
				});
				yield* installer.Rollback({
					operation_id: "rollback-operation",
					rollback_id: first.rollback_id!,
				});
				yield* installer.Rollback({
					operation_id: "rollback-retry",
					rollback_id: first.rollback_id!,
				});
				return { changed_retry, first, installed_before_rollback, second };
			}).pipe(Effect.provide(make_local_routine_installer_layer({ install_root }))),
		);
		expect(receipt.first).toEqual(receipt.second);
		expect(receipt.changed_retry._tag).toBe("Failure");
		expect(receipt.installed_before_rollback).toBe("Local instructions");
		expect(receipt.first.rollback_id).toMatch(/^rollback_/);
		expect((await readdir(install_root)).filter((entry) => !entry.startsWith("."))).toEqual([]);
	});

	it("rejects changed content and traversal without materializing it", async () => {
		const source = await TemporaryRoot();
		const install_root = join(await TemporaryRoot(), "installed");
		await writeFile(join(source, "SKILL.md"), "original");
		const hash = createHash("sha256").update("original").digest("hex");
		const inspection = {
			artifact_refs: [`sha256:${hash}:SKILL.md`],
			candidate_id: "routine_local",
			compatibility: [],
			content_hashes: { "../escape": hash, "SKILL.md": hash },
			description: "Local",
			display_name: "Local",
			exported_commands: [],
			files: [{ path: "SKILL.md", required: true }],
			instructions: "original",
			permissions: [],
			rollback_available: true,
			source: { kind: "local" as const, locator: source },
			trust: "local" as const,
			version: "1",
		};
		const exit = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* Effect.exit(
					(yield* RoutineInstaller).Install({
						inspection,
						operation_id: "traversal",
						scope: { kind: "global" },
					}),
				);
			}).pipe(Effect.provide(make_local_routine_installer_layer({ install_root }))),
		);
		expect(exit._tag).toBe("Failure");
		await expect(access(join(install_root, "..", "escape"))).rejects.toBeDefined();
	});

	it("keeps npx acquisition inert and uses bounded argv-only discovery", async () => {
		let spawns = 0;
		let launch: { readonly args: ReadonlyArray<string>; readonly command: string } | undefined;
		const encoder = new TextEncoder();
		let output = JSON.stringify({
			candidates: [
				{
					description: "Candidate",
					files: [{ path: "SKILL.md", required: true }],
					name: "candidate",
					source_locator: "package:candidate",
					version: "1.0.0",
				},
			],
		});
		const factory = Layer.succeed(EngineProcessFactory, {
			Spawn: (input) =>
				Effect.sync(() => {
					spawns += 1;
					launch = input;
					return {
						Close: Effect.void,
						EndInput: Effect.void,
						Exit: Effect.succeed({ code: 0, signal: null }),
						Kill: () => Effect.void,
						Stderr: (async function* () {})(),
						Stdout: (async function* () {
							yield encoder.encode(output);
						})(),
						Write: () => Effect.void,
					};
				}),
		});
		const layer = make_npx_skills_process_adapter_layer({
			args: ["--no-install", "skills", "inspect", "--json"],
			command: "npx",
		}).pipe(Layer.provide(factory));
		await Effect.runPromise(Effect.void.pipe(Effect.provide(layer)));
		expect(spawns).toBe(0);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* NpxSkillsAdapter).Discover({
					package_spec: "@example/skills",
					scope: { kind: "global" },
				});
			}).pipe(Effect.provide(layer)),
		);
		expect(spawns).toBe(1);
		expect(launch).toEqual({
			args: ["--no-install", "skills", "inspect", "--json", "@example/skills"],
			command: "npx",
		});
		expect(result.candidates).toHaveLength(1);
		output = "{}";
		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					return yield* (yield* NpxSkillsAdapter).Discover({
						package_spec: "@example/malformed",
						scope: { kind: "global" },
					});
				}).pipe(Effect.provide(layer)),
			),
		).rejects.toBeDefined();
		const bounded = make_npx_skills_process_adapter_layer({
			args: ["--no-install", "skills", "inspect", "--json"],
			command: "npx",
			max_output_bytes: 1,
		}).pipe(Layer.provide(factory));
		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					return yield* (yield* NpxSkillsAdapter).Discover({
						package_spec: "@example/oversized",
						scope: { kind: "global" },
					});
				}).pipe(Effect.provide(bounded)),
			),
		).rejects.toBeDefined();
	});
});
