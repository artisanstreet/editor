const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash, randomUUID } = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

if (process.platform !== "win32" || process.env.ARTISAN_RUN_NATIVE_REPLACE_SMOKE !== "1") {
	process.exit(0);
}

const module_root = path.resolve(process.argv[2]);
const { getNativeBuildDescriptor, NativeBoundedRegularFileStore } = require(module_root);
const roots = new Set();
const stores = new Set();
const cleanup_failures = [];
const encoder = new TextEncoder();
const receipt_key = new Uint8Array(32).fill(0x41);
const wrong_receipt_key = new Uint8Array(32).fill(0x42);

assert.equal(
	getNativeBuildDescriptor().testHooksEnabled,
	process.env.ARTISAN_RUN_NATIVE_RACE_SMOKE === "1",
);

if (process.env.ARTISAN_RUN_NATIVE_RACE_SMOKE === "1") {
	assert.ok(Number(process.env.UV_THREADPOOL_SIZE) >= 2);
}

function make_root() {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), "artisan-native-replace-"));
	roots.add(root);
	return root;
}

function cleanup_root(root) {
	try {
		if (fs.existsSync(root)) {
			execFileSync("attrib.exe", ["-h", "-r", path.join(root, "*"), "/s", "/d"], {
				stdio: "ignore",
			});
		}
	} catch {}
	let failure;

	try {
		fs.rmSync(root, { force: true, recursive: true });
	} catch (error) {
		failure = error;
	}

	if (fs.existsSync(root)) {
		cleanup_failures.push(
			new Error(`native replacement smoke could not remove ${path.basename(root)}`, {
				cause: failure,
			}),
		);

		return;
	}

	roots.delete(root);
}

function open_store(root, key = receipt_key) {
	const store = new NativeBoundedRegularFileStore(root, key);
	stores.add(store);

	return store;
}

function close_store(store) {
	if (!store) return;

	store.close();
	stores.delete(store);
}

function cleanup_all() {
	for (const store of stores) close_store(store);
	for (const root of roots) cleanup_root(root);
}

function take_cleanup_failures() {
	return cleanup_failures.splice(0, cleanup_failures.length);
}

function assert_opaque(error, forbidden = []) {
	for (const value of [module_root, ...roots, ...forbidden].filter(Boolean)) {
		assert.equal(String(error?.message ?? error).includes(value), false);
	}
	return true;
}

function rejects_opaque(operation, forbidden = []) {
	return assert.rejects(Promise.resolve().then(operation), (error) =>
		assert_opaque(error, forbidden),
	);
}

function namespace(operation_id, relative_path) {
	const canonical_path = relative_path.replaceAll("\\", "/").toLowerCase();

	return createHash("sha256")
		.update(encoder.encode(operation_id))
		.update(new Uint8Array([0]))
		.update(encoder.encode(canonical_path))
		.digest("hex");
}

function artifact_paths(root, operation_id, relative_path) {
	const value = namespace(operation_id, relative_path);
	const compact = value.slice(0, 32);
	const formatted = `${compact.slice(0, 8)}-${compact.slice(8, 12)}-${compact.slice(12, 16)}-${compact.slice(16, 20)}-${compact.slice(20)}`;
	const parent = path.dirname(path.join(root, relative_path));

	return {
		namespace: value,
		stage: path.join(parent, `.artisan-conditional-${value}.stage`),
		backup: path.join(parent, `.artisan-conditional-${value}.backup-${formatted}`),
	};
}

function options(
	operation_id,
	relative_path = "document.txt",
	expected = "old",
	replacement = "new",
	maximum_bytes = 1024,
) {
	return {
		expected: typeof expected === "string" ? encoder.encode(expected) : expected,
		replacement: typeof replacement === "string" ? encoder.encode(replacement) : replacement,
		maximumBytes: maximum_bytes,
		operationId: operation_id,
		path: relative_path,
	};
}

function artifact_names(root) {
	const names = [];

	function visit(directory) {
		for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
			const entry_path = path.join(directory, entry.name);
			if (entry.isDirectory()) {
				visit(entry_path);
				continue;
			}
			if (entry.name.startsWith(".artisan-conditional-")) names.push(entry_path);
		}
	}

	visit(root);
	return names;
}

function write_file(root, relative_path, value) {
	const target = path.join(root, relative_path);
	fs.mkdirSync(path.dirname(target), { force: true, recursive: true });
	fs.writeFileSync(target, value);
	return target;
}

async function race_results(race_name, attempt, operations) {
	process.env.ARTISAN_NATIVE_TEST_REPLACE_BARRIER = randomUUID();

	try {
		const settled = await Promise.allSettled(
			operations.map(({ name, run }) =>
				Promise.resolve()
					.then(run)
					.catch((cause) => {
						const detail = cause instanceof Error ? cause.stack : String(cause);

						throw new Error(
							`replacement race ${race_name} attempt ${attempt + 1} ${name} failed\n${detail}`,
							{ cause },
						);
					}),
			),
		);
		const failures = settled
			.filter((result) => result.status === "rejected")
			.map((result) => result.reason);

		if (failures.length > 0) {
			throw new AggregateError(failures, "replacement race rejected an operation");
		}

		return settled.map((result) => result.value);
	} finally {
		delete process.env.ARTISAN_NATIVE_TEST_REPLACE_BARRIER;
	}
}

function set_attributes(file, flags) {
	const args = flags.includes("H") ? ["+h"] : ["-h"];
	args.push(flags.includes("R") ? "+r" : "-r");
	execFileSync("attrib.exe", [...args, file], { stdio: "ignore" });
}

function read_attributes(file) {
	return execFileSync("attrib.exe", [file], { encoding: "utf8" }).trim();
}

function read_security_descriptor(file) {
	const snapshot = path.join(os.tmpdir(), `artisan-native-acl-${randomUUID()}.txt`);

	try {
		execFileSync("icacls.exe", [file, "/save", snapshot, "/c", "/q"], { stdio: "ignore" });

		return fs.readFileSync(snapshot, "utf16le");
	} finally {
		fs.rmSync(snapshot, { force: true });
	}
}

function protect_security_descriptor(file) {
	execFileSync("icacls.exe", [file, "/inheritancelevel:d", "/c", "/q"], {
		stdio: "ignore",
	});
}

async function basic_replacement() {
	const root = make_root();
	const operation_id = "basic-operation";
	const relative_path = "document.txt";
	const target = write_file(root, relative_path, "old");
	const artifacts = artifact_paths(root, operation_id, relative_path);
	const store = open_store(root);

	try {
		assert.equal(
			await store.replaceRegularFile(options(operation_id, relative_path)),
			"Replaced",
		);
		assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
		assert.equal(
			await store.replaceRegularFile(options(operation_id, relative_path)),
			"AlreadyReplaced",
		);
		close_store(store);

		const reopened = open_store(root);
		assert.equal(
			await reopened.replaceRegularFile(options(operation_id, relative_path)),
			"AlreadyReplaced",
		);
		await reopened.finalizeRegularFileReplacement(options(operation_id, relative_path));
		await reopened.finalizeRegularFileReplacement(options(operation_id, relative_path));
		assert.equal(fs.existsSync(artifacts.stage), false);
		assert.equal(fs.existsSync(artifacts.backup), false);
		close_store(reopened);
	} finally {
		close_store(store);
		cleanup_root(root);
	}
}

async function changed_and_validation() {
	const root = make_root();
	const store = open_store(root);
	try {
		write_file(root, "wrong.txt", "external");
		assert.equal(await store.replaceRegularFile(options("missing", "missing.txt")), "Changed");
		assert.equal(await store.replaceRegularFile(options("wrong", "wrong.txt")), "Changed");
		assert.equal(artifact_names(root).length, 0);
		write_file(root, "equal.txt", "same");
		assert.equal(
			await store.replaceRegularFile(options("equal", "equal.txt", "same", "same")),
			"AlreadyReplaced",
		);

		for (const maximum_bytes of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 32]) {
			await rejects_opaque(() =>
				store.replaceRegularFile(
					options("invalid", "wrong.txt", "old", "new", maximum_bytes),
				),
			);
		}
		await rejects_opaque(() =>
			store.replaceRegularFile(options("invalid", "wrong.txt", "1234", "new", 3)),
		);
		await rejects_opaque(() =>
			store.replaceRegularFile(options("invalid", "wrong.txt", "old", "1234", 3)),
		);
		for (const operation_id of ["", "bad\0id", "x".repeat(4097)]) {
			await rejects_opaque(
				() => store.replaceRegularFile(options(operation_id, "wrong.txt")),
				[operation_id],
			);
		}
		for (const relative_path of [
			"",
			".",
			"..",
			"../wrong.txt",
			".\\wrong.txt",
			"wrong.txt:stream",
			".artisan-trash/wrong.txt",
			".artisan-conditional-x/wrong.txt",
			"wrong.txt\0suffix",
		]) {
			await rejects_opaque(
				() => store.replaceRegularFile(options("invalid-path", relative_path)),
				[relative_path],
			);
		}

		assert.deepEqual(fs.readFileSync(path.join(root, "wrong.txt")), Buffer.from("external"));
		assert.equal(artifact_names(root).length, 0);
	} finally {
		close_store(store);
		cleanup_root(root);
	}
}

async function seed_receipt(operation_id, relative_path = "document.txt") {
	const root = make_root();
	const target = write_file(root, relative_path, "old");
	const store = open_store(root);
	assert.equal(await store.replaceRegularFile(options(operation_id, relative_path)), "Replaced");
	close_store(store);

	return { root, target, artifacts: artifact_paths(root, operation_id, relative_path) };
}

async function authenticated_recovery() {
	{
		const { root, target, artifacts } = await seed_receipt("recover-publish");
		try {
			fs.rmSync(target);
			const store = open_store(root);
			assert.equal(await store.replaceRegularFile(options("recover-publish")), "Replaced");
			assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
			assert.equal(fs.existsSync(artifacts.stage), true);
			assert.equal(fs.existsSync(artifacts.backup), true);
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}

	{
		const { root, target, artifacts } = await seed_receipt("recover-restore");
		try {
			fs.rmSync(target);
			fs.rmSync(artifacts.stage);
			const store = open_store(root);
			assert.equal(await store.replaceRegularFile(options("recover-restore")), "Changed");
			assert.deepEqual(fs.readFileSync(target), Buffer.from("old"));
			assert.equal(artifact_names(root).length, 0);
			assert.equal(await store.replaceRegularFile(options("after-restore")), "Replaced");
			await store.finalizeRegularFileReplacement(options("after-restore"));
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}

	{
		const { root, target, artifacts } = await seed_receipt("recover-stage-only-missing");
		try {
			fs.rmSync(target);
			fs.rmSync(artifacts.backup);
			const store = open_store(root);
			await rejects_opaque(() =>
				store.replaceRegularFile(options("recover-stage-only-missing")),
			);
			assert.equal(fs.existsSync(artifacts.stage), true);
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}

	{
		const { root, target, artifacts } = await seed_receipt("recover-stage-external");
		try {
			fs.rmSync(target);
			fs.rmSync(artifacts.backup);
			write_file(root, "document.txt", "external");
			const store = open_store(root);
			assert.equal(
				await store.replaceRegularFile(options("recover-stage-external")),
				"Changed",
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("external"));
			assert.equal(fs.existsSync(artifacts.stage), false);
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}

	{
		const { root, target, artifacts } = await seed_receipt("recover-published-external");
		try {
			fs.rmSync(target);
			write_file(root, "document.txt", "external");
			const store = open_store(root);
			await rejects_opaque(() =>
				store.replaceRegularFile(options("recover-published-external")),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("external"));
			assert.equal(fs.existsSync(artifacts.stage), true);
			assert.equal(fs.existsSync(artifacts.backup), true);
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}

	{
		const { root, target, artifacts } = await seed_receipt("recover-missing-backup");
		try {
			fs.rmSync(artifacts.backup);
			const store = open_store(root);
			await rejects_opaque(() => store.replaceRegularFile(options("recover-missing-backup")));
			await rejects_opaque(() =>
				store.finalizeRegularFileReplacement(options("recover-missing-backup")),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
			assert.equal(fs.existsSync(artifacts.stage), true);
			close_store(store);
		} finally {
			cleanup_root(root);
		}
	}
}

async function corrupt_and_collision() {
	for (const kind of ["stage", "backup"]) {
		const root = make_root();
		const operation_id = `corrupt-${kind}`;
		const relative_path = "document.txt";
		const target = write_file(root, relative_path, "old");
		const artifacts = artifact_paths(root, operation_id, relative_path);
		write_file(
			root,
			path.relative(root, kind === "stage" ? artifacts.stage : artifacts.backup),
			"corrupt",
		);
		const store = open_store(root);
		try {
			await rejects_opaque(() =>
				store.replaceRegularFile(options(operation_id, relative_path)),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("old"));
			assert.equal(
				fs.existsSync(kind === "stage" ? artifacts.stage : artifacts.backup),
				true,
			);
		} finally {
			close_store(store);
			cleanup_root(root);
		}
	}

	for (const kind of ["stage", "backup"]) {
		const root = make_root();
		const operation_id = `unmarked-${kind}`;
		const relative_path = "document.txt";
		const target = write_file(root, relative_path, "old");
		const artifacts = artifact_paths(root, operation_id, relative_path);
		fs.writeFileSync(
			kind === "stage" ? artifacts.stage : artifacts.backup,
			kind === "stage" ? "new" : "old",
		);
		const store = open_store(root);
		try {
			await rejects_opaque(() =>
				store.replaceRegularFile(options(operation_id, relative_path)),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("old"));
			assert.deepEqual(
				fs.readFileSync(kind === "stage" ? artifacts.stage : artifacts.backup),
				Buffer.from(kind === "stage" ? "new" : "old"),
			);
		} finally {
			close_store(store);
			cleanup_root(root);
		}
	}
}

async function authenticated_tamper_and_replay() {
	for (const kind of ["stage", "backup"]) {
		const operation_id = `tampered-${kind}`;
		const { root, target, artifacts } = await seed_receipt(operation_id);
		const artifact = kind === "stage" ? artifacts.stage : artifacts.backup;
		const tampered = Buffer.from(`tampered-${kind}`);
		let store;

		try {
			fs.writeFileSync(artifact, tampered);
			store = open_store(root);
			await rejects_opaque(() => store.replaceRegularFile(options(operation_id)));
			await rejects_opaque(() => store.finalizeRegularFileReplacement(options(operation_id)));
			assert.deepEqual(fs.readFileSync(artifact), tampered);
			assert.equal(fs.existsSync(artifacts.stage), true);
			assert.equal(fs.existsSync(artifacts.backup), true);
			assert.deepEqual(
				fs.readFileSync(target),
				kind === "stage" ? tampered : Buffer.from("new"),
			);
		} finally {
			close_store(store);
			cleanup_root(root);
		}
	}

	{
		const source_operation = "operation-replay-source";
		const replay_operation = "operation-replay-destination";
		const relative_path = "document.txt";
		const { root, target, artifacts } = await seed_receipt(source_operation, relative_path);
		const replay = artifact_paths(root, replay_operation, relative_path);
		let store;

		try {
			fs.linkSync(artifacts.stage, replay.stage);
			store = open_store(root);
			await rejects_opaque(() =>
				store.replaceRegularFile(options(replay_operation, relative_path)),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
			assert.equal(fs.existsSync(replay.stage), true);
		} finally {
			close_store(store);
			cleanup_root(root);
		}
	}

	{
		const operation_id = "path-replay";
		const source_path = "source.txt";
		const replay_path = "replayed.txt";
		const { root, artifacts } = await seed_receipt(operation_id, source_path);
		const replay_target = write_file(root, replay_path, "old");
		const replay = artifact_paths(root, operation_id, replay_path);
		let store;

		try {
			fs.linkSync(artifacts.stage, replay.stage);
			store = open_store(root);
			await rejects_opaque(() =>
				store.replaceRegularFile(options(operation_id, replay_path)),
			);
			assert.deepEqual(fs.readFileSync(replay_target), Buffer.from("old"));
			assert.equal(fs.existsSync(replay.stage), true);
		} finally {
			close_store(store);
			cleanup_root(root);
		}
	}

	{
		const operation_id = "root-replay";
		const relative_path = "document.txt";
		const source = await seed_receipt(operation_id, relative_path);
		const destination_root = make_root();
		const destination_target = write_file(destination_root, relative_path, "old");
		const replay = artifact_paths(destination_root, operation_id, relative_path);
		let store;

		try {
			fs.linkSync(source.artifacts.stage, replay.stage);
			store = open_store(destination_root);
			await rejects_opaque(() =>
				store.replaceRegularFile(options(operation_id, relative_path)),
			);
			assert.deepEqual(fs.readFileSync(destination_target), Buffer.from("old"));
			assert.equal(fs.existsSync(replay.stage), true);
		} finally {
			close_store(store);
			cleanup_root(destination_root);
			cleanup_root(source.root);
		}
	}
}

async function wrong_key() {
	{
		const root = make_root();
		const operation_id = "wrong-key-operation";
		const target = write_file(root, "document.txt", "old");
		const artifacts = artifact_paths(root, operation_id, "document.txt");
		const owner = open_store(root);
		let wrong_key_store;

		try {
			assert.equal(await owner.replaceRegularFile(options(operation_id)), "Replaced");
			wrong_key_store = open_store(root, wrong_receipt_key);
			await rejects_opaque(() => wrong_key_store.replaceRegularFile(options(operation_id)));
			await rejects_opaque(() =>
				wrong_key_store.finalizeRegularFileReplacement(options(operation_id)),
			);
			assert.deepEqual(fs.readFileSync(target), Buffer.from("new"));
			assert.equal(fs.existsSync(artifacts.stage), true);
			assert.equal(fs.existsSync(artifacts.backup), true);
			await owner.finalizeRegularFileReplacement(options(operation_id));
		} finally {
			close_store(wrong_key_store);
			close_store(owner);
			cleanup_root(root);
		}
	}

	{
		const operation_id = "wrong-key-recovery";
		const { root, target, artifacts } = await seed_receipt(operation_id);
		let wrong_key_store;
		let owner;

		try {
			fs.rmSync(target);
			wrong_key_store = open_store(root, wrong_receipt_key);
			await rejects_opaque(() => wrong_key_store.replaceRegularFile(options(operation_id)));
			assert.equal(fs.existsSync(target), false);
			assert.equal(fs.existsSync(artifacts.stage), true);
			assert.equal(fs.existsSync(artifacts.backup), true);
			close_store(wrong_key_store);
			owner = open_store(root);
			assert.equal(await owner.replaceRegularFile(options(operation_id)), "Replaced");
			await owner.finalizeRegularFileReplacement(options(operation_id));
		} finally {
			close_store(wrong_key_store);
			close_store(owner);
			cleanup_root(root);
		}
	}
}

async function preserve_windows_metadata() {
	const root = make_root();
	const relative_path = "attributes.txt";
	const target = write_file(root, relative_path, "old");
	set_attributes(target, "H R ");
	protect_security_descriptor(target);
	const before_attributes = read_attributes(target);
	const before_security = read_security_descriptor(target);
	assert.match(before_security, /D:P/u);
	const operation_id = "metadata-operation";
	const store = open_store(root);
	try {
		assert.equal(
			await store.replaceRegularFile(options(operation_id, relative_path)),
			"Replaced",
		);
		assert.equal(read_attributes(target), before_attributes);
		assert.equal(read_security_descriptor(target), before_security);
		await store.finalizeRegularFileReplacement(options(operation_id, relative_path));
		assert.equal(read_attributes(target), before_attributes);
		assert.equal(read_security_descriptor(target), before_security);
	} finally {
		close_store(store);
		cleanup_root(root);
	}
}

async function concurrent_replacement() {
	for (let attempt = 0; attempt < 5; attempt += 1) {
		const root = make_root();
		const relative_path = "concurrent.txt";
		const concurrent_old = Buffer.alloc(1024 * 1024, 7);
		const concurrent_new = Buffer.alloc(1024 * 1024, 8);
		write_file(root, relative_path, concurrent_old);
		try {
			const results = await race_results("same operation", attempt, [
				{
					name: "first caller",
					run: () =>
						run_race_operation(
							root,
							"same-operation",
							relative_path,
							concurrent_old,
							concurrent_new,
						),
				},
				{
					name: "second caller",
					run: () =>
						run_race_operation(
							root,
							"same-operation",
							relative_path,
							concurrent_old,
							concurrent_new,
						),
				},
			]);
			assert.deepEqual(results.slice().sort(), ["AlreadyReplaced", "Replaced"]);
			assert.equal(artifact_names(root).length, 2);
		} finally {
			cleanup_root(root);
		}

		const competing_root = make_root();
		const competing_target = write_file(competing_root, relative_path, concurrent_old);
		try {
			const results = await race_results("competing operations", attempt, [
				{
					name: "first operation",
					run: () =>
						run_race_operation(
							competing_root,
							"first-operation",
							relative_path,
							concurrent_old,
							concurrent_new,
						),
				},
				{
					name: "second operation",
					run: () =>
						run_race_operation(
							competing_root,
							"second-operation",
							relative_path,
							concurrent_old,
							concurrent_new,
						),
				},
			]);
			assert.deepEqual(results.slice().sort(), ["Changed", "Replaced"]);
			assert.deepEqual(fs.readFileSync(competing_target), concurrent_new);
			assert.equal(artifact_names(competing_root).length, 2);
		} finally {
			cleanup_root(competing_root);
		}
	}
}

async function run_race_operation(root, operation_id, relative_path, expected, replacement) {
	const store = open_store(root);

	try {
		return await store.replaceRegularFile(
			options(operation_id, relative_path, expected, replacement, replacement.length),
		);
	} finally {
		close_store(store);
	}
}

async function close_and_namespace() {
	const root = make_root();
	const store = open_store(root);
	const large_old = Buffer.alloc(16 * 1024 * 1024, 7);
	const large_new = Buffer.alloc(16 * 1024 * 1024, 8);
	write_file(root, "large.bin", large_old);
	const in_flight = store.replaceRegularFile(
		options("large-operation", "large.bin", large_old, large_new, large_new.length),
	);
	close_store(store);
	assert.equal(await in_flight, "Replaced");
	await rejects_opaque(() => store.replaceRegularFile(options("after-close", "large.bin")));
	await rejects_opaque(() =>
		store.finalizeRegularFileReplacement(options("large-operation", "large.bin")),
	);
	cleanup_root(root);

	const finalization_root = make_root();
	const finalization_operation = "in-flight-finalization";
	write_file(finalization_root, "finalize.txt", "old");
	const finalization_store = open_store(finalization_root);
	try {
		assert.equal(
			await finalization_store.replaceRegularFile(
				options(finalization_operation, "finalize.txt"),
			),
			"Replaced",
		);
		const finalization = finalization_store.finalizeRegularFileReplacement(
			options(finalization_operation, "finalize.txt"),
		);
		close_store(finalization_store);
		await finalization;
		assert.equal(artifact_names(finalization_root).length, 0);
	} finally {
		close_store(finalization_store);
		cleanup_root(finalization_root);
	}

	const namespace_root = make_root();
	const operation_id = "namespace-operation";
	const first_path = "Case\\File.txt";
	const retry_path = "case/File.txt";
	const finalize_path = "CASE\\FILE.TXT";
	write_file(namespace_root, first_path, "old");
	const namespace_store = open_store(namespace_root);
	try {
		assert.equal(
			await namespace_store.replaceRegularFile(options(operation_id, first_path)),
			"Replaced",
		);
		assert.equal(
			await namespace_store.replaceRegularFile(options(operation_id, retry_path)),
			"AlreadyReplaced",
		);
		await namespace_store.finalizeRegularFileReplacement(options(operation_id, finalize_path));
		assert.equal(artifact_names(namespace_root).length, 0);
		assert.equal(namespace(operation_id, first_path), namespace(operation_id, retry_path));
	} finally {
		close_store(namespace_store);
		cleanup_root(namespace_root);
	}
}

async function main() {
	const phases =
		process.env.ARTISAN_RUN_NATIVE_RACE_SMOKE === "1"
			? [["concurrent replacement", concurrent_replacement]]
			: [
					["basic replacement", basic_replacement],
					["changed and validation", changed_and_validation],
					["authenticated recovery", authenticated_recovery],
					["corrupt artifacts and collision", corrupt_and_collision],
					["authenticated tamper and replay", authenticated_tamper_and_replay],
					["wrong key", wrong_key],
					["Windows metadata", preserve_windows_metadata],
					["close and namespace", close_and_namespace],
				];

	for (const [name, operation] of phases) {
		let operation_error;

		try {
			await operation();
		} catch (error) {
			operation_error = error;
		} finally {
			cleanup_all();
		}

		const cleanup_errors = take_cleanup_failures();

		if (operation_error || cleanup_errors.length > 0) {
			throw new AggregateError(
				[operation_error, ...cleanup_errors].filter(Boolean),
				`native replacement smoke phase failed: ${name}`,
			);
		}
	}
}

main().catch((error) => {
	cleanup_all();
	const cleanup_errors = take_cleanup_failures();
	console.error(
		cleanup_errors.length > 0
			? new AggregateError([error, ...cleanup_errors], "native replacement smoke failed")
			: error,
	);
	process.exitCode = 1;
});
