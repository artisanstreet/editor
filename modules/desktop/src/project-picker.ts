import { createHash } from "node:crypto";
import { realpath, stat } from "node:fs/promises";
import { basename, normalize, resolve } from "node:path";

import { Data, Effect, Option, Schema } from "effect";

import { ProjectRef } from "@artisan/protocol";

/** The only native-dialog facts accepted before they cross into Artisan data. */
export const DesktopDirectoryDialogResult = Schema.Struct({
	canceled: Schema.Boolean,
	filePaths: Schema.Array(Schema.String),
});

export type DesktopDirectoryDialogResult = typeof DesktopDirectoryDialogResult.Type;

/** Keeps Electron's dialog capability injectable for deterministic desktop tests. */
export interface DesktopProjectDirectoryDialog {
	readonly ShowOpenDialog: () => Effect.Effect<unknown, unknown>;
}

/** Identifies a failed native folder selection without exposing an ambient filesystem capability. */
export class DesktopProjectPickerError extends Data.TaggedError("DesktopProjectPickerError")<{
	readonly cause: unknown;
	readonly operation: "decode" | "resolve" | "validate";
}> {}

const normalize_path = (path: string) => {
	const normalized_path = normalize(resolve(path)).replaceAll("\\", "/");

	return normalized_path.replace(/(?<!^[A-Za-z]:)\/$/, "");
};

const project_ref = (root_path: string) => {
	const normalized_root_path = normalize_path(root_path);

	return {
		display_name: basename(normalized_root_path) || normalized_root_path,
		project_id: `project_${createHash("sha256").update(normalized_root_path).digest("hex")}`,
		root_path: normalized_root_path,
	};
};

/**
 * Runs one user-initiated native folder picker and returns a validated project reference.
 *
 * Cancellation is an ordinary `Option.none`; invalid dialog results and vanished/non-directory
 * paths stay in the typed failure channel. The identifier algorithm deliberately matches the
 * backend ProjectLocator, while the backend remains responsible for later Git-root refinement.
 */
export const SelectDesktopProjectDirectory = (dialog: DesktopProjectDirectoryDialog) =>
	Effect.gen(function* () {
		const result = yield* dialog.ShowOpenDialog().pipe(
			Effect.flatMap(
				Schema.decodeUnknownEffect(DesktopDirectoryDialogResult, {
					onExcessProperty: "error",
				}),
			),
			Effect.mapError((cause) =>
				cause instanceof DesktopProjectPickerError
					? cause
					: new DesktopProjectPickerError({ cause, operation: "decode" }),
			),
		);

		if (result.canceled) {
			return Option.none<ProjectRef>();
		}

		const selected_path = result.filePaths.at(0);

		if (
			result.filePaths.length !== 1 ||
			selected_path === undefined ||
			selected_path.trim().length === 0
		) {
			return yield* Effect.fail(
				new DesktopProjectPickerError({
					cause: new Error("The project picker must return exactly one directory"),
					operation: "validate",
				}),
			);
		}

		const canonical_path = yield* Effect.tryPromise({
			try: async () => (await realpath(selected_path)).toString(),
			catch: (cause) => new DesktopProjectPickerError({ cause, operation: "resolve" }),
		});
		const metadata = yield* Effect.tryPromise({
			try: () => stat(canonical_path),
			catch: (cause) => new DesktopProjectPickerError({ cause, operation: "resolve" }),
		});

		if (!metadata.isDirectory()) {
			return yield* Effect.fail(
				new DesktopProjectPickerError({
					cause: new Error("The selected project path is not a directory"),
					operation: "validate",
				}),
			);
		}

		const project = yield* Schema.decodeUnknownEffect(ProjectRef, {
			onExcessProperty: "error",
		})(project_ref(canonical_path)).pipe(
			Effect.mapError(
				(cause) => new DesktopProjectPickerError({ cause, operation: "validate" }),
			),
		);

		return Option.some(project);
	});
