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

export const ProjectDirectoryList = Schema.Struct({
	directories: Schema.Array(ProjectDirectoryEntry).check(Schema.isMaxLength(256)),
	parent_directory_id: Schema.optional(ProjectDirectoryId),
});
export type ProjectDirectoryList = typeof ProjectDirectoryList.Type;

export const ProjectDirectorySelectInput = Schema.Struct({
	directory_id: ProjectDirectoryId,
});
export type ProjectDirectorySelectInput = typeof ProjectDirectorySelectInput.Type;
