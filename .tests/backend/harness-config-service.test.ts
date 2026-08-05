import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer, Option, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_config_file_store_layer } from "../../modules/backend/src/harness-config/file-store";
import {
	CodexRequestUserInput,
	MakeHarnessConfigRegistryLayer,
	type HarnessConfigKey,
} from "../../modules/backend/src/harness-config/keys";
import { HarnessConfig, HarnessConfigLive } from "../../modules/backend/src/harness-config/service";

const directories: Array<string> = [];

const make_directory = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-harness-config-"));

	directories.push(directory);

	return directory;
};

/** Builds the service over a real temporary Codex config, as production composes it. */
const make_service = async (root: string) => {
	const registry = MakeHarnessConfigRegistryLayer({
		targets: [
			{
				backups_directory: join(root, "backups"),
				format: "toml",
				harness_id: "codex",
				path: join(root, "config.toml"),
			},
		],
	}).pipe(Layer.orDie);
	const layer = HarnessConfigLive.pipe(
		Layer.provideMerge(registry),
		Layer.provideMerge(make_config_file_store_layer()),
	);

	return Effect.runPromise(Effect.service(HarnessConfig).pipe(Effect.provide(layer)));
};

const run = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(effect as Effect.Effect<A>);

const run_exit = <A, E>(effect: Effect.Effect<A, E>) => Effect.runPromise(Effect.exit(effect));

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })),
	);
});

describe("HarnessConfig", () => {
	it("reads a harness default as an absent value", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		const reading = await run(config.Read(CodexRequestUserInput));

		expect(reading.value).toStrictEqual(Option.none());
		expect(reading.document_hash).toStrictEqual(Option.none());
		expect(reading.activation).toBe("new_threads");
		expect(reading.key_id).toBe("codex:features.default_mode_request_user_input");
	});

	it("previews a change without touching the filesystem", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		const change = await run(config.Diff(CodexRequestUserInput, Option.some(true)));

		expect(change._tag).toBe("Change");

		if (change._tag !== "Change") return;

		expect(change.creates_document).toBe(true);
		expect(change.writes_backup).toBe(false);
		expect(change.next).toStrictEqual(Option.some(true));
		await expect(readFile(join(root, "config.toml"), "utf8")).rejects.toThrow();
	});

	it("writes the declared key into a document that did not exist", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		const outcome = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Written");
		expect(outcome.reading.value).toStrictEqual(Option.some(true));

		const written = await readFile(join(root, "config.toml"), "utf8");

		expect(written).toContain("default_mode_request_user_input = true");
	});

	it("preserves unrelated keys, comments, and credentials, and backs up the original", async () => {
		const root = await make_directory();
		const original = [
			"# Sander's configuration",
			'model = "gpt-5.2-codex"',
			"",
			"[mcp_servers.private]",
			'api_key = "sk-do-not-touch"',
			"",
		].join("\n");
		await writeFile(join(root, "config.toml"), original, "utf8");

		const config = await make_service(root);
		const outcome = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Written");

		const written = await readFile(join(root, "config.toml"), "utf8");

		expect(written).toContain("# Sander's configuration");
		expect(written).toContain('api_key = "sk-do-not-touch"');
		expect(written).toContain("default_mode_request_user_input = true");

		if (outcome._tag !== "Written") return;

		expect(Option.isSome(outcome.backup_path)).toBe(true);

		const backup = await readFile(Option.getOrThrow(outcome.backup_path), "utf8");

		expect(backup).toBe(original);
	});

	it("reports an unchanged write without republishing the document", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		await run(config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }));

		const before = await readFile(join(root, "config.toml"), "utf8");
		const outcome = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-2" }),
		);

		expect(outcome._tag).toBe("Unchanged");
		expect(await readFile(join(root, "config.toml"), "utf8")).toBe(before);
	});

	it("removes the key so the harness returns to its own default", async () => {
		const root = await make_directory();
		await writeFile(
			join(root, "config.toml"),
			'model = "gpt-5.2-codex"\n\n[features]\ndefault_mode_request_user_input = true\nother = true\n',
			"utf8",
		);

		const config = await make_service(root);
		const outcome = await run(
			config.Delete(CodexRequestUserInput, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Written");
		expect(outcome.reading.value).toStrictEqual(Option.none());

		const written = await readFile(join(root, "config.toml"), "utf8");

		expect(written).not.toContain("default_mode_request_user_input");
		expect(written).toContain('model = "gpt-5.2-codex"');
		expect(written.match(/other = true/g)).toHaveLength(1);
	});

	it("reports deleting an already-absent key as unchanged", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		const outcome = await run(
			config.Delete(CodexRequestUserInput, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Unchanged");
	});

	it("refuses a key the registry does not declare", async () => {
		const root = await make_directory();
		const config = await make_service(root);
		const undeclared: HarnessConfigKey<boolean> = {
			activation: "immediate",
			description: "Not owned by Artisan.",
			harness_id: "codex",
			path: ["sandbox_workspace_write", "network_access"],
			schema: Schema.Boolean,
		};
		const exit = await run_exit(
			config.Write(undeclared, true, { operation_id: "operation-1" }),
		);

		expect(exit._tag).toBe("Failure");
		await expect(readFile(join(root, "config.toml"), "utf8")).rejects.toThrow();
	});

	it("refuses a harness with no configured target", async () => {
		const root = await make_directory();
		const registry = MakeHarnessConfigRegistryLayer({
			keys: [CodexRequestUserInput],
			targets: [],
		}).pipe(Layer.orDie);
		const layer = HarnessConfigLive.pipe(
			Layer.provideMerge(registry),
			Layer.provideMerge(make_config_file_store_layer()),
		);
		const config = await Effect.runPromise(
			Effect.service(HarnessConfig).pipe(Effect.provide(layer)),
		);
		const exit = await run_exit(config.Read(CodexRequestUserInput));

		expect(exit._tag).toBe("Failure");
		expect(root).toBeTruthy();
	});

	it("reports a stored value that disagrees with the declared shape", async () => {
		const root = await make_directory();
		await writeFile(
			join(root, "config.toml"),
			'[features]\ndefault_mode_request_user_input = "yes"\n',
			"utf8",
		);

		const config = await make_service(root);
		const exit = await run_exit(config.Read(CodexRequestUserInput));

		expect(exit._tag).toBe("Failure");
	});

	/**
	 * A retry of the same operation must not publish twice or strand a second
	 * backup, so a lost receipt can be replayed safely.
	 */
	it("replays one operation id without writing a second time", async () => {
		const root = await make_directory();
		await writeFile(join(root, "config.toml"), 'model = "gpt-5.2-codex"\n', "utf8");

		const config = await make_service(root);
		const first = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);
		const replay = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);

		expect(first._tag).toBe("Written");
		expect(replay._tag).toBe("Unchanged");
		expect(replay.reading.value).toStrictEqual(Option.some(true));
	});

	it("carries an edit made between two writes rather than reverting it", async () => {
		const root = await make_directory();
		const target = join(root, "config.toml");
		await writeFile(target, 'model = "gpt-5.2-codex"\n', "utf8");

		const config = await make_service(root);
		const stale = await run(config.Read(CodexRequestUserInput));

		expect(Option.isSome(stale.document_hash)).toBe(true);

		await writeFile(target, 'model = "gpt-5.2-codex"\napproval_policy = "never"\n', "utf8");

		const outcome = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Written");
		expect(await readFile(target, "utf8")).toContain('approval_policy = "never"');
		expect(await readFile(target, "utf8")).toContain("default_mode_request_user_input = true");
	});

	/**
	 * The window that matters is between observing the bytes and publishing
	 * over them. The file plane's hook reproduces it deterministically: an
	 * external write lands after the fence is claimed, so the publication must
	 * be refused and reported rather than clobbering the newer document.
	 */
	it("refuses to publish over a document that changed mid-write", async () => {
		const root = await make_directory();
		const target = join(root, "config.toml");
		await writeFile(target, 'model = "gpt-5.2-codex"\n', "utf8");

		const registry = MakeHarnessConfigRegistryLayer({
			targets: [
				{
					backups_directory: join(root, "backups"),
					format: "toml",
					harness_id: "codex",
					path: target,
				},
			],
		}).pipe(Layer.orDie);
		const files = make_config_file_store_layer({
			before_backup: async () => {
				await writeFile(target, 'model = "raced-by-another-process"\n', "utf8");
			},
		});
		const config = await Effect.runPromise(
			Effect.service(HarnessConfig).pipe(
				Effect.provide(
					HarnessConfigLive.pipe(Layer.provideMerge(registry), Layer.provideMerge(files)),
				),
			),
		);
		const outcome = await run(
			config.Write(CodexRequestUserInput, true, { operation_id: "operation-1" }),
		);

		expect(outcome._tag).toBe("Changed");
		expect(await readFile(target, "utf8")).toBe('model = "raced-by-another-process"\n');
	});
});
