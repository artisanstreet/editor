import { TomlDocument } from "@decimalturn/toml-patch";
import { Data, Effect, Option, Schema } from "effect";

/**
 * Format-aware structured editing for harness config documents.
 *
 * Every operation is a pure transform over document text: read one owned key,
 * set it, or remove it. Comments, formatting, unknown keys, profiles, and
 * unrelated credentials in the surrounding document are preserved, because
 * Artisan only ever owns individual leaf keys inside files the user also owns.
 *
 * Errors deliberately carry no document content. A harness config file sits
 * beside credentials and private configuration, so a parse failure reports the
 * operation and format only.
 */

/** Names a structured config format Artisan can edit in place. */
export type ConfigDocumentFormat = "json" | "toml";

/** Addresses one key by its nested path, for example `["features", "enabled"]`. */
export type ConfigKeyPath = readonly [string, ...ReadonlyArray<string>];

/** Reports a document operation failure without exposing surrounding config. */
export class ConfigDocumentError extends Data.TaggedError("ConfigDocumentError")<{
	readonly cause: unknown;
	readonly format: ConfigDocumentFormat;
	readonly operation: "parse" | "patch" | "verify";
}> {}

type MutableRecord = Record<string, unknown>;

const is_record = (value: unknown): value is MutableRecord =>
	typeof value === "object" && value !== null && !Array.isArray(value);

/** Reads one nested key, treating any non-record ancestor as absence. */
const read_at_path = (root: unknown, path: ConfigKeyPath): Option.Option<unknown> => {
	let current: unknown = root;

	for (const segment of path) {
		if (!is_record(current) || !Object.hasOwn(current, segment)) {
			return Option.none();
		}

		current = current[segment];
	}

	return current === undefined ? Option.none() : Option.some(current);
};

/**
 * Returns a copy with one nested key set, creating intermediate tables.
 *
 * A non-record ancestor is replaced rather than merged: a scalar standing where
 * Artisan's declared key path expects a table means the document disagrees with
 * the declaration, and the declaration is what the write was authorized for.
 */
const set_at_path = (
	root: unknown,
	head: string,
	rest: ReadonlyArray<string>,
	value: unknown,
): MutableRecord => {
	const base = is_record(root) ? { ...root } : {};
	const [next, ...remaining] = rest;

	base[head] = next === undefined ? value : set_at_path(base[head], next, remaining, value);

	return base;
};

/**
 * Returns a copy with one nested key removed.
 *
 * A table left empty by the removal is kept. Artisan cannot tell whether the
 * user created that table for their own keys earlier, and deleting a container
 * we did not create is a larger edit than the one that was authorized.
 */
const delete_at_path = (
	root: unknown,
	head: string,
	rest: ReadonlyArray<string>,
): MutableRecord => {
	const base = is_record(root) ? { ...root } : {};
	const [next, ...remaining] = rest;

	if (next === undefined) {
		delete base[head];

		return base;
	}

	if (is_record(base[head])) {
		base[head] = delete_at_path(base[head], next, remaining);
	}

	return base;
};

const set_in_document = (root: unknown, path: ConfigKeyPath, value: unknown) => {
	const [head, ...rest] = path;

	return set_at_path(root, head, rest, value);
};

const delete_in_document = (root: unknown, path: ConfigKeyPath) => {
	const [head, ...rest] = path;

	return delete_at_path(root, head, rest);
};

const parse_toml = (content: string) =>
	Effect.try({
		catch: (cause) => new ConfigDocumentError({ cause, format: "toml", operation: "parse" }),
		try: () => new TomlDocument(content, { integersAsBigInt: false }),
	});

/**
 * Detects the indentation the document already uses so patches match it.
 *
 * A document Artisan creates falls back to two spaces rather than Artisan's own
 * tab convention: the file lives in the harness's directory and the user will
 * read it alongside the harness's own output.
 */
const detect_json_indent = (content: string) => {
	const match = /\n([ \t]+)\S/.exec(content);
	const indent = match?.[1];

	return indent === undefined ? "  " : indent;
};

/**
 * Decodes an arbitrary JSON config document.
 *
 * The shape is unknown by design — Artisan reads one declared key out of a file
 * the harness owns — so the schema is `Unknown` and the declared key's own
 * schema validates the value that is actually extracted.
 *
 * JSON decoding rejects comments, so a JSONC document fails here instead of
 * being silently rewritten without the user's annotations.
 */
const JsonDocument = Schema.fromJsonString(Schema.Unknown);

const parse_json = (content: string) =>
	content.trim() === ""
		? Effect.succeed<unknown>({})
		: Schema.decodeUnknownEffect(JsonDocument)(content).pipe(
				Effect.mapError(
					(cause) =>
						new ConfigDocumentError({ cause, format: "json", operation: "parse" }),
				),
			);

const serialize_json = (value: unknown, source: string) => {
	const trailing = source === "" || source.endsWith("\n") ? "\n" : "";

	return `${JSON.stringify(value, undefined, detect_json_indent(source))}${trailing}`;
};

/** Reads one owned key from a document without altering it. */
export const ReadConfigValue = (
	format: ConfigDocumentFormat,
	content: string,
	path: ConfigKeyPath,
) =>
	Effect.gen(function* () {
		const parsed =
			format === "toml"
				? ((yield* parse_toml(content)).toJsObject as unknown)
				: yield* parse_json(content);

		return read_at_path(parsed, path);
	});

/**
 * Applies one owned change and verifies it round-trips.
 *
 * The verification re-reads the serialized document rather than trusting the
 * patch: a formatter that dropped or coerced the value must fail the write
 * instead of publishing a document that disagrees with Artisan's record.
 */
const WriteConfigDocument = (
	format: ConfigDocumentFormat,
	content: string,
	path: ConfigKeyPath,
	produce: Effect.Effect<string, ConfigDocumentError>,
	expected: Option.Option<unknown>,
) =>
	Effect.gen(function* () {
		const patched = yield* produce;
		const verified = yield* ReadConfigValue(format, patched, path);

		if (!Option.isSome(expected)) {
			return Option.isNone(verified)
				? patched
				: yield* new ConfigDocumentError({
						cause: new Error("The patched document still carries the removed key"),
						format,
						operation: "verify",
					});
		}

		/**
		 * Structural equality is the honest check here: TOML and JSON both
		 * round-trip scalars and containers by value, not by reference.
		 */
		return JSON.stringify(Option.getOrUndefined(verified)) === JSON.stringify(expected.value)
			? patched
			: yield* new ConfigDocumentError({
					cause: new Error("The patched document did not preserve the requested value"),
					format,
					operation: "verify",
				});
	});

const patch_toml = (content: string, next: (parsed: unknown) => MutableRecord) =>
	Effect.gen(function* () {
		const document = yield* parse_toml(content);
		const updated = next(document.toJsObject as unknown);

		return yield* Effect.try({
			catch: (cause) =>
				new ConfigDocumentError({ cause, format: "toml", operation: "patch" }),
			try: () => {
				document.patch(updated);

				return document.toTomlString;
			},
		});
	});

const patch_json = (content: string, next: (parsed: unknown) => MutableRecord) =>
	Effect.gen(function* () {
		const parsed = yield* parse_json(content);

		return yield* Effect.try({
			catch: (cause) =>
				new ConfigDocumentError({ cause, format: "json", operation: "patch" }),
			try: () => serialize_json(next(parsed), content),
		});
	});

/** Sets one owned key, preserving every unrelated part of the document. */
export const SetConfigValue = (
	format: ConfigDocumentFormat,
	content: string,
	path: ConfigKeyPath,
	value: unknown,
) => {
	const next = (parsed: unknown) => set_in_document(parsed, path, value);

	return WriteConfigDocument(
		format,
		content,
		path,
		format === "toml" ? patch_toml(content, next) : patch_json(content, next),
		Option.some(value),
	);
};

/**
 * Removes one owned key so the harness falls back to its own default.
 *
 * Requires `@decimalturn/toml-patch` 3.0.1 or newer. Before that release,
 * removing a key whose value matched an untouched sibling's was misread as a
 * rename onto that sibling, emitting the key twice and producing invalid TOML
 * (DecimalTurn/toml-patch#262) — and two harness flags set to `true` in one
 * table were enough to hit it. Downgrading reintroduces silent config
 * corruption; the removal tests are the guard.
 *
 * Every spelling of a key is handled: table header, dotted root key, dotted key
 * inside a table, and inline table. Removing an emptied intermediate container
 * along with its leaf makes `patch()` throw, which is one more reason only the
 * leaf is ever removed — the library elides an emptied table itself.
 */
export const DeleteConfigValue = (
	format: ConfigDocumentFormat,
	content: string,
	path: ConfigKeyPath,
) => {
	const next = (parsed: unknown) => delete_in_document(parsed, path);

	return WriteConfigDocument(
		format,
		content,
		path,
		format === "toml" ? patch_toml(content, next) : patch_json(content, next),
		Option.none(),
	);
};
