const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const { createHash, randomUUID } = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

if (process.platform !== "win32" || process.env.ARTISAN_RUN_NATIVE_CRASH_SMOKE !== "1") {
	process.exit(0);
}

const module_root = path.resolve(process.argv[2]);
const { getNativeBuildDescriptor, NativeBoundedRegularFileStore } = require(module_root);
const worker = path.join(__dirname, "native-replace-crash-worker.cjs");
const encoder = new TextEncoder();
const receipt_key = new Uint8Array(32).fill(0x41);
const roots = new Set();

assert.equal(getNativeBuildDescriptor().testHooksEnabled, true);

function make_root() {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), "artisan-native-crash-"));
	roots.add(root);

	return root;
}

function cleanup_root(root) {
	if (fs.existsSync(root)) {
		spawnSync("attrib.exe", ["-h", "-r", path.join(root, "*"), "/s", "/d"]);
	}
	fs.rmSync(root, { force: true, recursive: true });
	assert.equal(fs.existsSync(root), false);
	roots.delete(root);
}

function options(operation_id, relative_path) {
	return {
		expected: encoder.encode("old"),
		replacement: encoder.encode("new"),
		maximumBytes: 1024,
		operationId: operation_id,
		path: relative_path,
	};
}

function artifact_paths(root, operation_id, relative_path) {
	const canonical_path = relative_path.replaceAll("\\", "/").toLowerCase();
	const namespace = createHash("sha256")
		.update(encoder.encode(operation_id))
		.update(new Uint8Array([0]))
		.update(encoder.encode(canonical_path))
		.digest("hex");
	const compact = namespace.slice(0, 32);
	const formatted = `${compact.slice(0, 8)}-${compact.slice(8, 12)}-${compact.slice(12, 16)}-${compact.slice(16, 20)}-${compact.slice(20)}`;
	const parent = path.dirname(path.join(root, relative_path));

	return {
		stage: path.join(parent, `.artisan-conditional-${namespace}.stage`),
		backup: path.join(parent, `.artisan-conditional-${namespace}.backup-${formatted}`),
	};
}

function crash_worker(mode, root, operation_id, relative_path, point) {
	const proof = path.join(os.tmpdir(), `artisan-native-crash-proof-${randomUUID()}`);
	const result = spawnSync(
		process.execPath,
		[worker, module_root, mode, root, operation_id, relative_path],
		{
			encoding: "utf8",
			env: {
				...process.env,
				ARTISAN_NATIVE_TEST_CRASH_POINT: point,
				ARTISAN_NATIVE_TEST_CRASH_PROOF: proof,
			},
			timeout: 30_000,
		},
	);

	try {
		assert.equal(result.error, undefined);
		assert.notEqual(result.status, 0);
		assert.doesNotMatch(result.stderr, /native crash hook did not terminate the worker/);
		assert.equal(fs.readFileSync(proof, "utf8"), point);
	} finally {
		fs.rmSync(proof, { force: true });
	}
}

async function replace_crash_windows() {
	const cases = [
		["creating-stage", "Replaced"],
		["stage-ready", "Replaced"],
		["backup-renamed", "Replaced"],
		["backup-marked", "Replaced"],
		["target-published", "AlreadyReplaced"],
	];

	for (const [point, expected_outcome] of cases) {
		const root = make_root();
		const operation_id = `crash-${point}`;
		const relative_path = "document.txt";
		const target = path.join(root, relative_path);
		const artifacts = artifact_paths(root, operation_id, relative_path);
		fs.writeFileSync(target, "old");

		try {
			crash_worker("replace", root, operation_id, relative_path, point);
			const store = new NativeBoundedRegularFileStore(root, receipt_key);
			try {
				assert.equal(
					await store.replaceRegularFile(options(operation_id, relative_path)),
					expected_outcome,
				);
				assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
				await store.finalizeRegularFileReplacement(options(operation_id, relative_path));
				assert.equal(fs.existsSync(artifacts.stage), false);
				assert.equal(fs.existsSync(artifacts.backup), false);
			} finally {
				store.close();
			}
		} finally {
			cleanup_root(root);
		}
	}
}

async function finalization_crash_windows() {
	for (const point of ["finalizing-marked", "backup-deleted", "stage-deleted"]) {
		const root = make_root();
		const operation_id = `crash-${point}`;
		const relative_path = "document.txt";
		const target = path.join(root, relative_path);
		const artifacts = artifact_paths(root, operation_id, relative_path);
		fs.writeFileSync(target, "old");
		const seed = new NativeBoundedRegularFileStore(root, receipt_key);

		try {
			assert.equal(
				await seed.replaceRegularFile(options(operation_id, relative_path)),
				"Replaced",
			);
			seed.close();
			crash_worker("finalize", root, operation_id, relative_path, point);
			const recovery = new NativeBoundedRegularFileStore(root, receipt_key);
			try {
				await recovery.finalizeRegularFileReplacement(options(operation_id, relative_path));
				assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
				assert.equal(fs.existsSync(artifacts.stage), false);
				assert.equal(fs.existsSync(artifacts.backup), false);
			} finally {
				recovery.close();
			}
		} finally {
			seed.close();
			cleanup_root(root);
		}
	}
}

async function restoration_crash_windows() {
	for (const point of ["restoring-marked", "target-restored", "restoration-backup-deleted"]) {
		const root = make_root();
		const operation_id = `crash-${point}`;
		const relative_path = "document.txt";
		const target = path.join(root, relative_path);
		const artifacts = artifact_paths(root, operation_id, relative_path);
		fs.writeFileSync(target, "old");
		const seed = new NativeBoundedRegularFileStore(root, receipt_key);

		try {
			assert.equal(
				await seed.replaceRegularFile(options(operation_id, relative_path)),
				"Replaced",
			);
			seed.close();
			fs.rmSync(target);
			fs.rmSync(artifacts.stage);
			crash_worker("replace", root, operation_id, relative_path, point);
			const recovery = new NativeBoundedRegularFileStore(root, receipt_key);
			try {
				assert.equal(
					await recovery.replaceRegularFile(options(operation_id, relative_path)),
					"Changed",
				);
				assert.deepEqual(fs.readFileSync(target), Buffer.from("old"));
				assert.equal(fs.existsSync(artifacts.stage), false);
				assert.equal(fs.existsSync(artifacts.backup), false);
			} finally {
				recovery.close();
			}
		} finally {
			seed.close();
			cleanup_root(root);
		}
	}
}

async function main() {
	await replace_crash_windows();
	await finalization_crash_windows();
	await restoration_crash_windows();
}

main().catch((error) => {
	for (const root of roots) {
		try {
			cleanup_root(root);
		} catch {}
	}
	console.error(error);
	process.exitCode = 1;
});
