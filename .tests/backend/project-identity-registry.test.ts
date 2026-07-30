import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { MakeSnowflakeIdLive } from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { Projects } from "../../modules/backend/src/persistence/tables";
import {
	ProjectIdentityRegistry,
	ProjectIdentityRegistryLive,
} from "../../modules/backend/src/projects/project-identity-registry";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_registry_layer() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-project-identity-"));

	temporary_directories.push(directory);

	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path: join(directory, "artisan.db"), migrations_path }),
		RuntimeMetadataLive,
		MakeSnowflakeIdLive(1).pipe(Layer.orDie),
	);

	return ProjectIdentityRegistryLive.pipe(Layer.provideMerge(infrastructure));
}

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((root) => rm(root, { force: true, recursive: true })),
	);
});

describe("ProjectIdentityRegistry", () => {
	it("mints a bare Snowflake once per root and answers from storage after", async () => {
		const layer = await make_registry_layer();

		const [first, second, other] = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* ProjectIdentityRegistry;

				return [
					yield* registry.Resolve("C:/repos/alpha"),
					yield* registry.Resolve("C:/repos/alpha"),
					yield* registry.Resolve("C:/repos/beta"),
				] as const;
			}).pipe(Effect.provide(layer)),
		);

		/** Bare digits, exactly like thread ids — no prefix, no hash. */
		expect(first).toMatch(/^\d+$/);
		expect(second).toBe(first);
		expect(other).toMatch(/^\d+$/);
		expect(other).not.toBe(first);
	});

	it("keeps an id independent of the attached project catalog", async () => {
		const layer = await make_registry_layer();

		const [before_attach, after_attach] = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* ProjectIdentityRegistry;
				const first = yield* registry.Resolve("C:/repos/gamma");
				const database = yield* Database;

				/** Attaching and detaching never re-mints: the registry row is the identity. */
				yield* database.client.insert(Projects).values({
					attached_at: "2026-07-01T00:00:00.000Z",
					display_name: "gamma",
					project_id: first,
					root_path: "C:/repos/gamma",
					updated_at: "2026-07-01T00:00:00.000Z",
				});
				yield* database.client.delete(Projects);

				return [first, yield* registry.Resolve("C:/repos/gamma")] as const;
			}).pipe(Effect.provide(layer)),
		);

		expect(after_attach).toBe(before_attach);
	});
});
