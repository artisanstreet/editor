import { Schema } from "effect";

import { Identifier } from "./common";

/** Opaque server-owned identity for one directory within an allowed root. */
export const ProjectDirectoryId = Identifier;
export type ProjectDirectoryId = typeof ProjectDirectoryId.Type;

/** A bounded directory entry safe to expose to browser clients. */
export const ProjectDirectoryEntry = Schema.Struct({
	directory_id: ProjectDirectoryId,
	display_name: Schema.NonEmptyString,
	has_children: Schema.Boolean,
	kind: Schema.Literals(["root", "directory"]),
});
export type ProjectDirectoryEntry = typeof ProjectDirectoryEntry.Type;

export const ProjectDirectoryListInput = Schema.Struct({
	parent_directory_id: Schema.optional(ProjectDirectoryId),
});
export type ProjectDirectoryListInput = typeof ProjectDirectoryListInput.Type;

/** The well-known user folders a picker offers as one-click shortcuts. */
export const ProjectDirectoryPlaceKind = Schema.Literals([
	"home",
	"desktop",
	"documents",
	"downloads",
	"music",
	"pictures",
	"videos",
]);
export type ProjectDirectoryPlaceKind = typeof ProjectDirectoryPlaceKind.Type;

export const ProjectDirectoryPlace = Schema.Struct({
	directory_id: ProjectDirectoryId,
	display_name: Schema.NonEmptyString,
	place: ProjectDirectoryPlaceKind,
});
export type ProjectDirectoryPlace = typeof ProjectDirectoryPlace.Type;

export const ProjectDirectoryList = Schema.Struct({
	directories: Schema.Array(ProjectDirectoryEntry).check(Schema.isMaxLength(256)),
	/** Plain file names in the listed directory — context for the picker, never selectable. */
	files: Schema.optional(Schema.Array(Schema.NonEmptyString).check(Schema.isMaxLength(256))),
	parent_directory_id: Schema.optional(ProjectDirectoryId),
	/** Present on every listing so the picker's sidebar never depends on which level is open. */
	places: Schema.optional(Schema.Array(ProjectDirectoryPlace).check(Schema.isMaxLength(16))),
});
export type ProjectDirectoryList = typeof ProjectDirectoryList.Type;

/**
 * The outcome of one Forge-owned operating-system folder dialog.
 *
 * A selected folder is represented only by the opaque directory identity
 * Forge minted for it. Host path data never crosses this boundary.
 */
export const ProjectDirectoryPickResult = Schema.Union([
	Schema.Struct({ status: Schema.Literal("cancelled") }),
	Schema.Struct({
		directory: ProjectDirectoryEntry,
		status: Schema.Literal("selected"),
	}),
]);
export type ProjectDirectoryPickResult = typeof ProjectDirectoryPickResult.Type;

export const ProjectDirectorySelectInput = Schema.Struct({
	directory_id: ProjectDirectoryId,
});
export type ProjectDirectorySelectInput = typeof ProjectDirectorySelectInput.Type;

/** Creates one directory inside an already-listed parent; the name is a single segment. */
export const ProjectDirectoryCreateInput = Schema.Struct({
	name: Schema.NonEmptyString,
	parent_directory_id: ProjectDirectoryId,
});
export type ProjectDirectoryCreateInput = typeof ProjectDirectoryCreateInput.Type;
