#!/usr/bin/env node
/**
 * NATIVE-0003: persisted database fixture matrix.
 *
 * Builds one SQLite database per historical Drizzle migration checkpoint by
 * replaying the real backend migration stack (the same
 * `make_database_layer` path production uses, including its pragma policy),
 * records a normalized content digest and pragma snapshot for every
 * checkpoint, and proves each fixture is a valid migration prefix by
 * migrating a copy of it forward to HEAD.
 *
 * The Rust Forge store packet opens these fixtures and must reproduce every
 * digest. Databases land in `.dist/native-db-matrix/`; the committed artifact
 * is `.tests/backend/generated/database-fixture-matrix.json`.
 *
 * Usage: node .scripts/native/database-fixture-matrix.ts [--out-dir DIR] [--limit N]
 */

import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "../..");
const migrationsSource = join(repositoryRoot, "modules/backend/drizzle");
// Deep import is deliberate: the tool reuses the exact production migration
// layer rather than paraphrasing its pragma policy.
const databaseModuleUrl = new URL(
	"../../modules/backend/src/persistence/database.ts",
	import.meta.url,
);

const PRAGMA_NAMES = [
	"journal_mode",
	"synchronous",
	"temp_store",
	"auto_vacuum",
	"cache_size",
	"journal_size_limit",
	"wal_autocheckpoint",
];

function parseArguments() {
	const arguments_ = process.argv.slice(2);
	const outDir = arguments_.includes("--out-dir")
		? resolve(arguments_[arguments_.indexOf("--out-dir") + 1])
		: join(repositoryRoot, ".dist/native-db-matrix");
	const limitArgumentIndex = arguments_.indexOf("--limit");
	const limit = limitArgumentIndex >= 0 ? Number(arguments_[limitArgumentIndex + 1]) : undefined;
	return { outDir, limit };
}

/** Migration directories sort chronologically by their timestamp prefix. */
function listMigrationTags() {
	return readdirSync(migrationsSource, { withFileTypes: true })
		.filter((entry) => entry.isDirectory())
		.map((entry) => entry.name)
		.sort();
}

function stagePrefix(tags, upToIndex, stagingRoot) {
	const staging = join(stagingRoot, `prefix-${String(upToIndex).padStart(4, "0")}`);
	mkdirSync(staging, { recursive: true });
	for (const tag of tags.slice(0, upToIndex + 1)) {
		cpSync(join(migrationsSource, tag), join(staging, tag), { recursive: true });
	}
	return staging;
}

/**
 * Migrates one fresh database file to the head of `migrationsPath` using the
 * production layer, then captures and returns the facts the manifest records.
 */
async function buildDatabase(databasePath, migrationsPath) {
	const { Database, make_database_layer } = await import(databaseModuleUrl);
	const { Effect } = await import("effect");

	const capture = Effect.gen(function* () {
		const { client } = yield* Database;

		const scalar = (query) =>
			Effect.gen(function* () {
				const [row] = yield* client.all(query);
				return Object.values(row)[0];
			});

		const journalTables = yield* client.all(
			"SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE '%drizzle%'",
		);
		const appliedCount =
			journalTables.length > 0
				? Number(yield* scalar(`SELECT COUNT(*) FROM "${journalTables[0].name}"`))
				: 0;

		const pragmas = {};
		for (const name of PRAGMA_NAMES) {
			pragmas[name] = String(yield* scalar(`PRAGMA ${name}`));
		}

		const [sqliteVersion] = yield* client.all("SELECT sqlite_version() AS version");
		const schemaDump = yield* client.all(
			"SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name",
		);

		yield* client.run("PRAGMA wal_checkpoint(TRUNCATE)");

		return {
			appliedCount,
			pragmas,
			schemaDump,
			sqliteVersion: String(sqliteVersion.version),
		};
	});

	return await Effect.runPromise(
		Effect.scoped(
			capture.pipe(
				Effect.provide(
					make_database_layer({
						database_path: databasePath,
						migrations_path: migrationsPath,
					}),
				),
			),
		),
	);
}

function contentDigest(facts) {
	return createHash("sha256")
		.update(
			JSON.stringify({
				appliedMigrations: facts.appliedCount,
				pragmas: facts.pragmas,
				schemaDump: facts.schemaDump,
				sqliteVersion: facts.sqliteVersion,
			}),
		)
		.digest("hex");
}

async function main() {
	const { outDir, limit } = parseArguments();
	const tags = listMigrationTags();
	const checkpointsToBuild = limit ? tags.slice(0, limit) : tags;
	mkdirSync(outDir, { recursive: true });
	const generatedDirectory = join(repositoryRoot, ".tests/backend/generated");
	mkdirSync(generatedDirectory, { recursive: true });
	const stagingRoot = join(outDir, ".staging");
	rmSync(stagingRoot, { recursive: true, force: true });

	// Ground truth: some drizzle-kit directories carry only a snapshot and no
	// SQL migration, so the applied count at HEAD can differ from the
	// directory count. Measure it instead of assuming.
	const headFacts = await buildDatabase(join(outDir, "head-probe.tmp.db"), migrationsSource);
	const headAppliedCount = headFacts.appliedCount;
	const headDigest = contentDigest(headFacts);
	rmSync(join(outDir, "head-probe.tmp.db"), { force: true });

	const checkpoints = [];
	for (const [index, tag] of checkpointsToBuild.entries()) {
		const staging = stagePrefix(tags, index, stagingRoot);
		const databasePath = join(outDir, `${tag}.db`);
		rmSync(databasePath, { force: true });

		const facts = await buildDatabase(databasePath, staging);

		// Forward proof: the fixture must be a valid migration prefix, so a
		// copy of it accepts every remaining migration and lands at HEAD.
		const forwardPath = `${databasePath}.forward.db`;
		rmSync(forwardPath, { force: true });
		cpSync(databasePath, forwardPath);
		const forwardFacts = await buildDatabase(forwardPath, migrationsSource);
		rmSync(forwardPath, { force: true });

		checkpoints.push({
			index,
			id: tag,
			appliedMigrations: facts.appliedCount,
			forwardMigratedToHead:
				forwardFacts.appliedCount === headAppliedCount &&
				contentDigest(forwardFacts) === headDigest,
			sqliteVersion: facts.sqliteVersion,
			pragmas: facts.pragmas,
			schemaObjects: facts.schemaDump.length,
			digest: contentDigest(facts),
			fileBytes: statSync(databasePath).size,
			file: `${tag}.db`,
		});
		console.log(`[${index + 1}/${checkpointsToBuild.length}] ${tag} ok`);
	}

	rmSync(stagingRoot, { recursive: true, force: true });

	writeFileSync(
		join(generatedDirectory, "database-fixture-matrix.json"),
		`${JSON.stringify(
			{
				schema: "artisan.database.fixture-matrix/1",
				migrationDirectoryCount: tags.length,
				appliedMigrationsAtHead: headAppliedCount,
				headDigest,
				sqliteVersion: headFacts.sqliteVersion,
				checkpoints,
			},
			null,
			"\t",
		)}\n`,
	);
	console.log(`matrix manifest -> ${join(generatedDirectory, "database-fixture-matrix.json")}`);
}

await main();
