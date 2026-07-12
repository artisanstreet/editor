import { Effect, Redacted } from "effect";
import { describe, expect, it } from "vitest";

import { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import {
	make_native_bounded_regular_file_store_layer,
	NativeBoundedRegularFileStoreInitializationError,
} from "../../modules/backend/src/filesystem/native-bounded-regular-file-store";

const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(7));

function make_module(
	options: {
		readonly descriptor?: unknown;
		readonly finalize?: (options: Record<string, unknown>) => Promise<unknown>;
		readonly invalid_instance?: boolean;
		readonly read?: (path: string, maximum_bytes: number) => Promise<unknown>;
		readonly replace?: (options: Record<string, unknown>) => Promise<unknown>;
		readonly throw_on_root?: string;
	} = {},
) {
	const constructed_roots: Array<string> = [];
	const key_copies: Array<Uint8Array> = [];
	const replace_options: Array<Record<string, unknown>> = [];
	const finalize_options: Array<Record<string, unknown>> = [];
	let close_count = 0;
	let load_count = 0;
	const descriptor =
		options.descriptor ??
		({
			architecture: "x86_64",
			operatingSystem: "windows",
			target: "x86_64-pc-windows-msvc",
			testHooksEnabled: false,
		} as const);

	class FakeNativeBoundedRegularFileStore {
		constructor(root: string, key: Uint8Array) {
			constructed_roots.push(root);
			key_copies.push(key);

			if (options.invalid_instance) {
				Object.defineProperty(this, "readRegularFile", { value: undefined });
			}

			if (root === options.throw_on_root) throw new Error("open failed");
		}

		close() {
			close_count += 1;
		}

		authorizeRoot(_candidate_root: string) {
			return Promise.resolve(true);
		}

		finalizeRegularFileReplacement(input: Record<string, unknown>) {
			finalize_options.push(input);

			return options.finalize?.(input) ?? Promise.resolve();
		}

		readRegularFile(path: string, maximum_bytes: number) {
			return options.read?.(path, maximum_bytes) ?? Promise.resolve(new Uint8Array([1, 2]));
		}

		replaceRegularFile(input: Record<string, unknown>) {
			replace_options.push(input);

			return options.replace?.(input) ?? Promise.resolve("Replaced");
		}
	}

	return {
		constructed_roots,
		finalize_options,
		get close_count() {
			return close_count;
		},
		key_copies,
		get load_count() {
			return load_count;
		},
		load_native_module: () => {
			load_count += 1;

			return {
				NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
				getNativeBuildDescriptor: () => descriptor,
			};
		},
		replace_options,
	};
}

function make_layer(module = make_module(), key = receipt_authentication_key) {
	return {
		layer: make_native_bounded_regular_file_store_layer({
			load_native_module: module.load_native_module,
			receipt_authentication_key: key,
			root: "C:\\workspace",
		}),
		module,
	};
}

function read_store(layer: ReturnType<typeof make_layer>["layer"]) {
	return Effect.scoped(Effect.service(BoundedRegularFileStore).pipe(Effect.provide(layer)));
}

function with_store<A, E, R>(
	layer: ReturnType<typeof make_layer>["layer"],
	use: (store: typeof BoundedRegularFileStore.Service) => Effect.Effect<A, E, R>,
) {
	return Effect.scoped(
		Effect.service(BoundedRegularFileStore).pipe(Effect.flatMap(use), Effect.provide(layer)),
	);
}

describe("native bounded regular file store", () => {
	it("keeps ordinary imports and unconstructed layers inert", async () => {
		const module = make_module();
		const { layer } = make_layer(module);

		expect(module.load_count).toBe(0);
		expect(layer).toBeTruthy();
	});

	it("rejects receipt keys and descriptors before root construction", async () => {
		const invalid_key_module = make_module();
		const invalid_key = Redacted.make(new Uint8Array(31));
		const invalid_key_failure = await Effect.runPromise(
			read_store(make_layer(invalid_key_module, invalid_key).layer).pipe(Effect.flip),
		);
		const invalid_descriptor_module = make_module({
			descriptor: {
				architecture: "x86_64",
				operatingSystem: "windows",
				target: "x86_64-pc-windows-msvc",
				testHooksEnabled: true,
			},
		});
		const invalid_descriptor_failure = await Effect.runPromise(
			read_store(make_layer(invalid_descriptor_module).layer).pipe(Effect.flip),
		);

		expect(invalid_key_failure).toBeInstanceOf(
			NativeBoundedRegularFileStoreInitializationError,
		);
		expect(invalid_key_module.load_count).toBe(0);
		expect(invalid_key_module.constructed_roots).toEqual([]);
		expect(invalid_descriptor_failure).toBeInstanceOf(
			NativeBoundedRegularFileStoreInitializationError,
		);
		expect(invalid_descriptor_module.constructed_roots).toEqual([]);
	});

	it.each([
		{
			architecture: "x86_64",
			operatingSystem: "windows",
			target: "x86_64-pc-windows-gnu",
			testHooksEnabled: false,
		},
		{
			architecture: "x86_64",
			operatingSystem: "windows",
			target: "x86_64-pc-windows-gnu",
			testHooksEnabled: true,
		},
	])("rejects non-production native descriptors %#", async (descriptor) => {
		const module = make_module({ descriptor });
		const failure = await Effect.runPromise(
			read_store(make_layer(module).layer).pipe(Effect.flip),
		);

		expect(failure).toBeInstanceOf(NativeBoundedRegularFileStoreInitializationError);
		expect(module.constructed_roots).toEqual([]);
	});

	it("closes a partially shaped store that acquired a root", async () => {
		const module = make_module({ invalid_instance: true });
		const failure = await Effect.runPromise(
			read_store(make_layer(module).layer).pipe(Effect.flip),
		);

		expect(failure).toBeInstanceOf(NativeBoundedRegularFileStoreInitializationError);
		expect(module.constructed_roots).toEqual(["C:\\workspace"]);
		expect(module.close_count).toBe(1);
	});

	it("adapts camelCase promises, options, results, and failures", async () => {
		const module = make_module({
			finalize: async () => {
				throw new Error("finalize failed");
			},
			read: async () => {
				throw new Error("read failed");
			},
			replace: async () => "invalid",
		});
		const replace_options = {
			expected: new Uint8Array([1]),
			maximum_bytes: 8,
			operation_id: "operation_1",
			path: "document.txt",
			replacement: new Uint8Array([2]),
		};
		const read_failure = await Effect.runPromise(
			with_store(make_layer(module).layer, (store) =>
				store.ReadRegularFile("document.txt", 8).pipe(Effect.flip),
			),
		);
		const replace_failure = await Effect.runPromise(
			with_store(make_layer(module).layer, (store) =>
				store.ReplaceRegularFile(replace_options).pipe(Effect.flip),
			),
		);
		const finalize_failure = await Effect.runPromise(
			with_store(make_layer(module).layer, (store) =>
				store.FinalizeRegularFileReplacement(replace_options).pipe(Effect.flip),
			),
		);

		expect(read_failure).toMatchObject({
			_tag: "BoundedRegularFileStoreError",
			operation: "read",
			path: "document.txt",
		});
		expect(replace_failure).toMatchObject({
			_tag: "BoundedRegularFileStoreError",
			operation: "replace",
			path: "document.txt",
		});
		expect(finalize_failure).toMatchObject({
			_tag: "BoundedRegularFileStoreError",
			operation: "finalize",
			path: "document.txt",
		});
		expect(module.replace_options).toEqual([
			{
				expected: new Uint8Array([1]),
				maximumBytes: 8,
				operationId: "operation_1",
				path: "document.txt",
				replacement: new Uint8Array([2]),
			},
		]);
		expect(module.finalize_options).toEqual([
			{
				expected: new Uint8Array([1]),
				maximumBytes: 8,
				operationId: "operation_1",
				path: "document.txt",
				replacement: new Uint8Array([2]),
			},
		]);
	});

	it("maps recognized native replacement results", async () => {
		const module = make_module({ replace: async () => "AlreadyReplaced" });
		const result = await Effect.runPromise(
			with_store(make_layer(module).layer, (store) =>
				store.ReplaceRegularFile({
					expected: new Uint8Array([1]),
					maximum_bytes: 8,
					operation_id: "operation_1",
					path: "document.txt",
					replacement: new Uint8Array([2]),
				}),
			),
		);

		expect(result).toEqual({ _tag: "AlreadyReplaced" });
	});

	it("rejects a fulfilled non-void native finalization", async () => {
		const module = make_module({ finalize: async () => "unexpected" });
		const failure = await Effect.runPromise(
			with_store(make_layer(module).layer, (store) =>
				store
					.FinalizeRegularFileReplacement({
						expected: new Uint8Array([1]),
						maximum_bytes: 8,
						operation_id: "operation_1",
						path: "document.txt",
						replacement: new Uint8Array([2]),
					})
					.pipe(Effect.flip),
			),
		);

		expect(failure).toMatchObject({
			_tag: "BoundedRegularFileStoreError",
			operation: "finalize",
			path: "document.txt",
		});
	});

	it("imports the public backend surface without acquiring the native layer", async () => {
		const backend = await import("../../modules/backend/src/index");

		expect(backend.make_native_bounded_regular_file_store_layer).toBeTypeOf("function");
	});

	it("closes each scoped store once and wipes only its temporary key copy", async () => {
		const key = new Uint8Array(32).fill(9);
		const module = make_module();

		await Effect.runPromise(read_store(make_layer(module, Redacted.make(key)).layer));

		expect(module.close_count).toBe(1);
		expect(module.key_copies).toHaveLength(1);
		expect([...module.key_copies[0]!]).toEqual(Array.from({ length: 32 }, () => 0));
		expect([...key]).toEqual(Array.from({ length: 32 }, () => 9));
	});
});
