import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	acquire_forge_database_lease,
	ForgeDatabaseAlreadyOwned,
} from "../../modules/forge/src/database-lease";

const roots: Array<string> = [];

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

const MakeDatabasePath = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-forge-lease-"));
	roots.push(root);
	return join(root, "artisan.db");
};

describe("Forge database lease", () => {
	it("rejects a second live owner and releases only its own lease", async () => {
		const database_path = await MakeDatabasePath();
		const first = await Effect.runPromise(acquire_forge_database_lease(database_path));

		await expect(
			Effect.runPromise(acquire_forge_database_lease(database_path)),
		).rejects.toBeInstanceOf(ForgeDatabaseAlreadyOwned);

		await Effect.runPromise(first.Release);
		const replacement = await Effect.runPromise(acquire_forge_database_lease(database_path));
		await Effect.runPromise(replacement.Release);
	});

	it("recovers a stale lease left by a terminated process", async () => {
		const database_path = await MakeDatabasePath();
		const lock_path = `${database_path}.artisan-forge.lock`;
		await writeFile(
			lock_path,
			JSON.stringify({ instance_id: "stale", pid: 2_147_483_647 }),
			"utf8",
		);

		const lease = await Effect.runPromise(acquire_forge_database_lease(database_path));
		const owner = JSON.parse(await readFile(lock_path, "utf8")) as {
			readonly instance_id: string;
		};
		expect(owner.instance_id).not.toBe("stale");
		await Effect.runPromise(lease.Release);
	});
});
