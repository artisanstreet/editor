import { createRequire } from "node:module";

import { Data, Effect, Layer, Redacted, Schema } from "effect";

import {
	BoundedRegularFileStore,
	BoundedRegularFileStoreError,
	type ReplaceRegularFileOptions,
	type ReplaceRegularFileResult,
} from "./bounded-regular-file-store";

const NativeBuildDescriptor = Schema.Struct({
	architecture: Schema.Literal("x86_64"),
	operatingSystem: Schema.Literal("windows"),
	target: Schema.Literal("x86_64-pc-windows-msvc"),
	testHooksEnabled: Schema.Literal(false),
});

type NativeBuildDescriptor = typeof NativeBuildDescriptor.Type;

interface NativeReplaceRegularFileOptions {
	readonly expected: Uint8Array;
	readonly maximumBytes: number;
	readonly operationId: string;
	readonly path: string;
	readonly replacement: Uint8Array;
}

interface NativeBoundedRegularFileStore {
	authorizeRoot(candidateRoot: string): Promise<unknown>;
	close(): unknown;
	finalizeRegularFileReplacement(options: NativeReplaceRegularFileOptions): Promise<unknown>;
	readRegularFile(path: string, maximumBytes: number): Promise<unknown>;
	replaceRegularFile(options: NativeReplaceRegularFileOptions): Promise<unknown>;
}

interface NativeBoundedRegularFileStoreConstructor {
	new (root: string, receiptAuthenticationKey: Uint8Array): NativeBoundedRegularFileStore;
}

interface NativeBoundedRegularFileStoreModule {
	NativeBoundedRegularFileStore: NativeBoundedRegularFileStoreConstructor;
	getNativeBuildDescriptor(): NativeBuildDescriptor;
}

/** Reports a rejected native module, descriptor, receipt key, or native store initialization. */
export class NativeBoundedRegularFileStoreInitializationError extends Data.TaggedError(
	"NativeBoundedRegularFileStoreInitializationError",
)<{
	readonly message: string;
}> {}

/** Configures one scoped native bounded regular-file store. */
export interface NativeBoundedRegularFileStoreOptions {
	readonly load_native_module?: () => unknown;
	readonly receipt_authentication_key: Redacted.Redacted<Uint8Array>;
	readonly root: string;
}

function initialization_error(message: string) {
	return new NativeBoundedRegularFileStoreInitializationError({ message });
}

function store_error(
	operation: BoundedRegularFileStoreError["operation"],
	path: string,
	cause: unknown,
) {
	return new BoundedRegularFileStoreError({ cause, operation, path });
}

function is_record(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null;
}

function is_native_module(value: unknown): value is NativeBoundedRegularFileStoreModule {
	return (
		is_record(value) &&
		typeof value.getNativeBuildDescriptor === "function" &&
		typeof value.NativeBoundedRegularFileStore === "function"
	);
}

function is_native_store(value: unknown): value is NativeBoundedRegularFileStore {
	return (
		is_record(value) &&
		typeof value.authorizeRoot === "function" &&
		typeof value.close === "function" &&
		typeof value.finalizeRegularFileReplacement === "function" &&
		typeof value.readRegularFile === "function" &&
		typeof value.replaceRegularFile === "function"
	);
}

function is_production_descriptor(value: unknown): value is NativeBuildDescriptor {
	return Schema.is(NativeBuildDescriptor)(value);
}

function close_if_possible(value: unknown) {
	if (!is_record(value) || typeof value.close !== "function") {
		return;
	}

	try {
		value.close();
	} catch {
		return;
	}
}

function LoadProductionNativeModule() {
	const require = createRequire(import.meta.url);
	const native_module_name = ["@artisan", "bounded-file-store-native"].join("/");

	return require(native_module_name);
}

function validate_receipt_authentication_key(
	receipt_authentication_key: Redacted.Redacted<Uint8Array>,
) {
	const key = Redacted.value(receipt_authentication_key);

	return key instanceof Uint8Array && key.length === 32;
}

function map_replace_result(path: string, result: unknown) {
	if (result === "Replaced" || result === "AlreadyReplaced" || result === "Changed") {
		return Effect.succeed({ _tag: result } satisfies ReplaceRegularFileResult);
	}

	return Effect.fail(store_error("replace", path, new Error("Native replace result is invalid")));
}

function native_replace_options(options: ReplaceRegularFileOptions) {
	return {
		expected: options.expected,
		maximumBytes: options.maximum_bytes,
		operationId: options.operation_id,
		path: options.path,
		replacement: options.replacement,
	} satisfies NativeReplaceRegularFileOptions;
}

function make_store_service(store: NativeBoundedRegularFileStore) {
	const ReadRegularFile = (path: string, maximum_bytes: number) =>
		Effect.tryPromise({
			catch: (cause) => store_error("read", path, cause),
			try: () => store.readRegularFile(path, maximum_bytes),
		}).pipe(
			Effect.flatMap((bytes) =>
				bytes instanceof Uint8Array && bytes.length <= maximum_bytes
					? Effect.succeed(bytes)
					: Effect.fail(
							store_error("read", path, new Error("Native read result is invalid")),
						),
			),
		);
	const ReplaceRegularFile = (options: ReplaceRegularFileOptions) =>
		Effect.tryPromise({
			catch: (cause) => store_error("replace", options.path, cause),
			try: () => store.replaceRegularFile(native_replace_options(options)),
		}).pipe(Effect.flatMap((result) => map_replace_result(options.path, result)));
	const FinalizeRegularFileReplacement = (options: ReplaceRegularFileOptions) =>
		Effect.tryPromise({
			catch: (cause) => store_error("finalize", options.path, cause),
			try: () => store.finalizeRegularFileReplacement(native_replace_options(options)),
		}).pipe(
			Effect.flatMap((result) =>
				result === undefined
					? Effect.void
					: Effect.fail(
							store_error(
								"finalize",
								options.path,
								new Error("Native finalization result is invalid"),
							),
						),
			),
		);

	return {
		FinalizeRegularFileReplacement,
		ReadRegularFile,
		ReplaceRegularFile,
	} satisfies typeof BoundedRegularFileStore.Service;
}

function make_root_authorizer(store: NativeBoundedRegularFileStore) {
	return (candidate_root: string) =>
		Effect.tryPromise({
			catch: () => new Error("Native root authorization failed"),
			try: () => store.authorizeRoot(candidate_root),
		}).pipe(
			Effect.orElseSucceed(() => false),
			Effect.map((authorized) => authorized === true),
		);
}

function AcquireNativeBoundedRegularFileStore(options: NativeBoundedRegularFileStoreOptions) {
	return Effect.try({
		catch: () => initialization_error("Native bounded file store initialization failed"),
		try: () => {
			if (!validate_receipt_authentication_key(options.receipt_authentication_key)) {
				throw new Error("Receipt authentication key is invalid");
			}

			const module =
				options.load_native_module === undefined
					? LoadProductionNativeModule()
					: options.load_native_module();

			if (!is_native_module(module)) {
				throw new Error("Native bounded file store module is invalid");
			}

			const descriptor = module.getNativeBuildDescriptor();

			if (!is_production_descriptor(descriptor)) {
				throw new Error("Native bounded file store descriptor is invalid");
			}

			const temporary_key = Uint8Array.from(
				Redacted.value(options.receipt_authentication_key),
			);
			let store: unknown;

			try {
				store = new module.NativeBoundedRegularFileStore(options.root, temporary_key);

				if (!is_native_store(store)) {
					throw new Error("Native bounded file store instance is invalid");
				}

				return store;
			} catch (cause) {
				close_if_possible(store);

				throw cause;
			} finally {
				temporary_key.fill(0);
			}
		},
	});
}

/** Acquires one scoped native store and adapts it to the bounded regular-file service. */
export function BuildNativeBoundedRegularFileStore(options: NativeBoundedRegularFileStoreOptions) {
	return BuildNativeBoundedRegularFileStoreWithRootAuthorization(options).pipe(
		Effect.map(({ store }) => store),
	);
}

/** Acquires one scoped native store with its opaque exact-root authorization operation. */
export function BuildNativeBoundedRegularFileStoreWithRootAuthorization(
	options: NativeBoundedRegularFileStoreOptions,
) {
	return Effect.acquireRelease(AcquireNativeBoundedRegularFileStore(options), (store) =>
		Effect.sync(() => void store.close()),
	).pipe(
		Effect.map((native_store) => ({
			AuthorizeRoot: make_root_authorizer(native_store),
			store: make_store_service(native_store),
		})),
	);
}

/** Builds the scoped production layer for one native bounded regular-file root. */
export function make_native_bounded_regular_file_store_layer(
	options: NativeBoundedRegularFileStoreOptions,
) {
	return Layer.effect(BoundedRegularFileStore, BuildNativeBoundedRegularFileStore(options));
}
