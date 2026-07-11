import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ModelBehaviourConfigFiles,
	make_model_behaviour_config_files_layer,
	type ModelBehaviourConfigFileHooks,
} from "../../modules/backend/src/model-behaviour/model-behaviour-config-files";
import {
	make_codex_model_behaviour_provider,
	type ModelBehaviourProviderObservation,
} from "../../modules/backend/src/model-behaviour/model-behaviour-provider";
import { make_codex_auto_compaction_mapping } from "../../modules/backend/src/model-behaviour/model-behaviour-registry";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(`${tmpdir()}/artisan model behaviour provider `);

	roots.push(root);

	return root;
}

async function make_provider(root: string, hooks: ModelBehaviourConfigFileHooks = {}) {
	const files = await Effect.runPromise(
		Effect.service(ModelBehaviourConfigFiles).pipe(
			Effect.provide(make_model_behaviour_config_files_layer(hooks)),
		),
	);

	return make_codex_model_behaviour_provider({
		backups_directory: join(root, "backups"),
		files,
		mapping: make_codex_auto_compaction_mapping({
			installed_version: "0.142.5",
			mapping_available: true,
		}),
		target_path: join(root, "codex home", "config.toml"),
	});
}

function apply_input(observation: ModelBehaviourProviderObservation, value = 250_000) {
	return {
		...(observation.document_hash === undefined
			? {}
			: { expected_document_hash: observation.document_hash }),
		expected_observed_hash: observation.observed_hash,
		operation_id: "model_behaviour_update_1",
		value: { type: "integer" as const, value },
	};
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("Codex Model Behaviour provider", () => {
	it("inspects only the owned value and never returns adjacent config content", async () => {
		const root = await make_root();
		const provider = await make_provider(root);
		const path = join(root, "codex home", "config.toml");
		const content =
			'# retained\napi_key = "secret-never-leaves-adapter"\nmodel_auto_compact_token_limit = 120000\n';

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, content, "utf8");

		const observation = await Effect.runPromise(provider.Inspect);

		expect(observation.value).toEqual({ type: "integer", value: 120_000 });
		expect(observation.document_exists).toBe(true);
		expect(observation.document_hash).toMatch(/^[a-f0-9]{64}$/);
		expect(observation.observed_hash).toMatch(/^[a-f0-9]{64}$/);
		expect(JSON.stringify(observation)).not.toContain("secret-never-leaves-adapter");
	});

	it("patches structurally, backs up exact bytes, and preserves unrelated values", async () => {
		const root = await make_root();
		const provider = await make_provider(root);
		const path = join(root, "codex home", "config.toml");
		const original =
			'# retained\r\napi_key = "secret"\r\nmodel_auto_compact_token_limit = 120000\r\n';

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, original, "utf8");

		const observed = await Effect.runPromise(provider.Inspect);
		const result = await Effect.runPromise(provider.Apply(apply_input(observed)));
		const updated = await fs.readFile(path, "utf8");

		expect(result._tag).toBe("Written");
		expect(result.observation.value).toEqual({ type: "integer", value: 250_000 });
		expect(updated).toContain("# retained");
		expect(updated).toContain('api_key = "secret"');
		expect(result._tag === "Written" ? result.backup_path : undefined).toBeDefined();
		await expect(
			fs.readFile(result._tag === "Written" ? result.backup_path! : "", "utf8"),
		).resolves.toBe(original);
	});

	it("treats a previously applied value as a write-free retry", async () => {
		const root = await make_root();
		let backup_attempts = 0;
		const provider = await make_provider(root, {
			before_backup: async () => {
				backup_attempts += 1;
			},
		});
		const path = join(root, "codex home", "config.toml");

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, "model_auto_compact_token_limit = 250000\n", "utf8");

		const observed = await Effect.runPromise(provider.Inspect);
		const result = await Effect.runPromise(provider.Apply(apply_input(observed)));

		expect(result._tag).toBe("AlreadyApplied");
		expect(backup_attempts).toBe(0);
		expect(await fs.readdir(root)).not.toContain("backups");
	});

	it("reports a raced external edit and preserves the external bytes", async () => {
		const root = await make_root();
		const path = join(root, "codex home", "config.toml");
		const raced = "model_auto_compact_token_limit = 300000\nexternal = true\n";
		const provider = await make_provider(root, {
			before_replace: async () => fs.writeFile(path, raced, "utf8"),
		});

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, "model_auto_compact_token_limit = 120000\n", "utf8");

		const observed = await Effect.runPromise(provider.Inspect);
		const result = await Effect.runPromise(provider.Apply(apply_input(observed)));

		expect(result).toMatchObject({
			_tag: "Changed",
			observation: { value: { type: "integer", value: 300_000 } },
		});
		expect(await fs.readFile(path, "utf8")).toBe(raced);
	});

	it("uses a new recoverable backup when the same operation sees a newer revision", async () => {
		const root = await make_root();
		const path = join(root, "codex home", "config.toml");
		const original = "model_auto_compact_token_limit = 120000\n";
		const raced = "model_auto_compact_token_limit = 300000\nexternal = true\n";
		let replacements = 0;
		const provider = await make_provider(root, {
			before_replace: async () => {
				replacements += 1;

				if (replacements === 1) {
					await fs.writeFile(path, raced, "utf8");
				}
			},
		});

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, original, "utf8");

		const observed = await Effect.runPromise(provider.Inspect);
		const first = await Effect.runPromise(provider.Apply(apply_input(observed)));

		expect(first._tag).toBe("Changed");

		const second = await Effect.runPromise(provider.Apply(apply_input(first.observation)));
		const backup_entries = await fs.readdir(join(root, "backups"));
		const backup_files = backup_entries.filter(
			(name) => name.includes(".original-") && !name.endsWith(".permissions.json"),
		);
		const permission_files = backup_entries.filter((name) =>
			name.endsWith(".permissions.json"),
		);
		const publication_anchors = backup_entries.filter((name) => name.includes(".replacement-"));
		const backup_contents = await Promise.all(
			backup_files.map((name) => fs.readFile(join(root, "backups", name), "utf8")),
		);

		expect(second._tag).toBe("Written");
		expect(backup_files).toHaveLength(2);
		expect(permission_files).toHaveLength(2);
		expect(publication_anchors).toHaveLength(2);
		expect(backup_contents).toEqual(expect.arrayContaining([original, raced]));
		expect(await fs.readFile(path, "utf8")).toContain(
			"model_auto_compact_token_limit = 250000",
		);
	});

	it("does not report synced when the published file changes before verification", async () => {
		const root = await make_root();
		const path = join(root, "codex home", "config.toml");
		const external = "model_auto_compact_token_limit = 300000\nexternal = true\n";
		const provider = await make_provider(root, {
			after_replace: async () => fs.writeFile(path, external, "utf8"),
		});

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, "model_auto_compact_token_limit = 120000\n", "utf8");

		const observed = await Effect.runPromise(provider.Inspect);
		const result = await Effect.runPromise(provider.Apply(apply_input(observed)));

		expect(result).toMatchObject({
			_tag: "Changed",
			observation: { value: { type: "integer", value: 300_000 } },
		});
		expect(await fs.readFile(path, "utf8")).toBe(external);
	});

	it("fails closed on malformed TOML and leaves the exact bytes untouched", async () => {
		const root = await make_root();
		const provider = await make_provider(root);
		const path = join(root, "codex home", "config.toml");
		const malformed = "broken = [\n";

		await fs.mkdir(join(root, "codex home"), { recursive: true });
		await fs.writeFile(path, malformed, "utf8");

		const result = await Effect.runPromise(provider.Inspect.pipe(Effect.exit));

		expect(result._tag).toBe("Failure");
		expect(JSON.stringify(result)).toContain("invalid_config");
		expect(await fs.readFile(path, "utf8")).toBe(malformed);
	});
});
