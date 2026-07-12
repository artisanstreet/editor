const assert = require("node:assert/strict");
const { randomUUID } = require("node:crypto");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

if (process.platform !== "win32" || process.env.ARTISAN_RUN_NATIVE_DIAGNOSTIC_SMOKE !== "1") {
	process.exit(0);
}

const receipt_key = new Uint8Array(32).fill(0x41);
const encoder = new TextEncoder();
const replacement_options = {
	expected: encoder.encode("old"),
	replacement: encoder.encode("new"),
	maximumBytes: 1024,
	operationId: "diagnostic-operation",
	path: "document.txt",
};

async function main() {
	let root;
	let store;
	let trace;

	try {
		const module_entry = path.resolve(process.argv[2]);
		const { getNativeBuildDescriptor, NativeBoundedRegularFileStore } = require(module_entry);
		root = fs.mkdtempSync(path.join(os.tmpdir(), "artisan-native-diagnostic-"));
		trace = path.join(os.tmpdir(), `artisan-native-trace-${randomUUID()}.log`);
		process.env.ARTISAN_NATIVE_TEST_TRACE = trace;
		assert.equal(getNativeBuildDescriptor().testHooksEnabled, true);
		fs.writeFileSync(path.join(root, "document.txt"), "old");
		store = new NativeBoundedRegularFileStore(root, receipt_key);
		assert.equal(await store.replaceRegularFile(replacement_options), "Replaced");
		await store.finalizeRegularFileReplacement(replacement_options);
	} catch (error) {
		if (fs.existsSync(trace)) {
			console.error("ARTISAN_NATIVE_TRACE");
			console.error(fs.readFileSync(trace, "utf8"));
		}

		throw error;
	} finally {
		delete process.env.ARTISAN_NATIVE_TEST_TRACE;
		store?.close();
		if (root) fs.rmSync(root, { force: true, recursive: true });
		if (trace) fs.rmSync(trace, { force: true });
	}
}

main().catch((error) => {
	console.error(error);
	process.exitCode = 1;
});
