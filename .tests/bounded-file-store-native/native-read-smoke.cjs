const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const module_root = process.argv[2];
const { NativeBoundedRegularFileStore } = require(module_root);
const receipt_key = new Uint8Array(32).fill(0x41);
const root = fs.mkdtempSync(path.join(os.tmpdir(), "artisan-native-read-"));
const outside = fs.mkdtempSync(path.join(os.tmpdir(), "artisan-native-outside-"));

function assert_opaque(error, forbidden = []) {
	for (const value of [root, outside, ...forbidden]) {
		assert.equal(error.message.includes(value), false);
	}

	return true;
}

function rejects_opaque(operation, forbidden = []) {
	return assert.rejects(Promise.resolve().then(operation), (error) => {
		return assert_opaque(error, forbidden);
	});
}

function throws_opaque(operation, forbidden = []) {
	assert.throws(operation, (error) => assert_opaque(error, forbidden));
}

function short_name(native_path) {
	const command = `for %I in ("${native_path.replaceAll('"', '""')}") do @echo %~sI`;
	const short_path = execFileSync(process.env.ComSpec ?? "cmd.exe", ["/d", "/s", "/c", command], {
		encoding: "utf8",
	}).trim();

	return path.basename(short_path);
}

async function main() {
	try {
		fs.writeFileSync(path.join(root, "bytes.bin"), Buffer.from([0xff, 0, 1]));
		fs.writeFileSync(path.join(root, "empty.bin"), Buffer.alloc(0));
		fs.writeFileSync(path.join(root, "large.bin"), Buffer.alloc(1024 * 1024 * 16, 7));
		fs.mkdirSync(path.join(root, "directory"));
		fs.mkdirSync(path.join(root, "nested"));
		fs.writeFileSync(path.join(root, "nested", "bytes.bin"), Buffer.from([2, 3]));
		fs.mkdirSync(path.join(root, ".artisan-trash"));
		fs.writeFileSync(path.join(root, ".artisan-trash", "private.bin"), "private");
		fs.mkdirSync(path.join(root, ".artisan-conditional-stage"));
		const artifact_name = `.artisan-conditional-${"a".repeat(64)}.stage`;
		const artifact_path = path.join(root, artifact_name);
		fs.writeFileSync(artifact_path, "artifact");
		fs.writeFileSync(path.join(outside, "outside.bin"), "outside");
		fs.symlinkSync(outside, path.join(root, "junction"), "junction");

		const store = new NativeBoundedRegularFileStore(root, receipt_key);
		assert.deepEqual(await store.readRegularFile("bytes.bin", 3), Buffer.from([0xff, 0, 1]));
		assert.deepEqual(await store.readRegularFile("empty.bin", 1), Buffer.alloc(0));
		assert.deepEqual(await store.readRegularFile("nested/bytes.bin", 2), Buffer.from([2, 3]));
		await rejects_opaque(() => store.readRegularFile("bytes.bin", 2));

		for (const maximum_bytes of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, 2 ** 32]) {
			await rejects_opaque(() => store.readRegularFile("bytes.bin", maximum_bytes));
		}

		await rejects_opaque(() => store.readRegularFile("missing.bin", 3), ["missing.bin"]);
		await rejects_opaque(() => store.readRegularFile("directory", 3), ["directory"]);

		for (const value of [
			"/bytes.bin",
			"../bytes.bin",
			".\\bytes.bin",
			"C:\\bytes.bin",
			"\\\\server\\share\\bytes.bin",
			"\\\\?\\C:\\bytes.bin",
			"bytes.bin:stream",
			"bytes.bin\0suffix",
			"bytes.bin.",
			"bytes.bin ",
			".artisan-trash/bytes.bin",
			".artisan-conditional-stage/bytes.bin",
			"junction/outside.bin",
		]) {
			await rejects_opaque(() => store.readRegularFile(value, 3), [value]);
		}

		const trash_alias = short_name(path.join(root, ".artisan-trash"));
		if (!trash_alias.toLowerCase().includes("artisan-trash")) {
			await rejects_opaque(
				() => store.readRegularFile(`${trash_alias}/private.bin`, 7),
				[trash_alias],
			);
		}

		const short_artifact_name = short_name(artifact_path);
		if (short_artifact_name.toLowerCase() !== artifact_name.toLowerCase()) {
			await rejects_opaque(
				() => store.readRegularFile(short_artifact_name, 8),
				[short_artifact_name],
			);
		}

		fs.linkSync(artifact_path, path.join(root, "artifact-alias.bin"));
		await rejects_opaque(
			() => store.readRegularFile("artifact-alias.bin", 8),
			["artifact-alias.bin"],
		);

		const writer = fs.openSync(path.join(root, "bytes.bin"), "r+");
		try {
			await rejects_opaque(() => store.readRegularFile("bytes.bin", 3));
		} finally {
			fs.closeSync(writer);
		}
		assert.deepEqual(await store.readRegularFile("bytes.bin", 3), Buffer.from([0xff, 0, 1]));

		const in_flight = store.readRegularFile("large.bin", 1024 * 1024 * 16);
		store.close();
		store.close();
		assert.equal((await in_flight).length, 1024 * 1024 * 16);
		await rejects_opaque(() => store.readRegularFile("bytes.bin", 3));

		for (const key of [new Uint8Array(0), new Uint8Array(31), new Uint8Array(33)]) {
			throws_opaque(() => new NativeBoundedRegularFileStore(root, key));
		}
		throws_opaque(() => new NativeBoundedRegularFileStore("", receipt_key));
		throws_opaque(() => new NativeBoundedRegularFileStore(".", receipt_key));
		throws_opaque(
			() => new NativeBoundedRegularFileStore("\\\\server\\share", receipt_key),
			["server", "share"],
		);
		throws_opaque(
			() => new NativeBoundedRegularFileStore(`${root}\0suffix`, receipt_key),
			["suffix"],
		);
		throws_opaque(
			() => new NativeBoundedRegularFileStore(path.join(root, "bytes.bin"), receipt_key),
			["bytes.bin"],
		);
		throws_opaque(
			() => new NativeBoundedRegularFileStore(path.join(root, "junction"), receipt_key),
			["junction"],
		);
	} finally {
		fs.rmSync(root, { recursive: true, force: true });
		fs.rmSync(outside, { recursive: true, force: true });
	}
}

main().catch((error) => {
	console.error(error);
	process.exitCode = 1;
});
