if (process.platform !== "win32" || process.env.ARTISAN_RUN_NATIVE_CRASH_SMOKE !== "1") {
	process.exit(0);
}

const { NativeBoundedRegularFileStore } = require(process.argv[2]);
const mode = process.argv[3];
const root = process.argv[4];
const operation_id = process.argv[5];
const relative_path = process.argv[6];
const encoder = new TextEncoder();
const receipt_key = new Uint8Array(32).fill(0x41);
const store = new NativeBoundedRegularFileStore(root, receipt_key);
const replacement_options = {
	expected: encoder.encode("old"),
	replacement: encoder.encode("new"),
	maximumBytes: 1024,
	operationId: operation_id,
	path: relative_path,
};

async function main() {
	if (mode === "replace") {
		await store.replaceRegularFile(replacement_options);
	} else if (mode === "finalize") {
		await store.finalizeRegularFileReplacement(replacement_options);
	} else {
		throw new Error("native crash smoke mode is invalid");
	}

	throw new Error("native crash hook did not terminate the worker");
}

main().catch((error) => {
	store.close();
	console.error(error);
	process.exitCode = 1;
});
