import { Schema } from "effect";

import { Identifier, PositiveInt } from "./common";
import { WorkspacePath } from "./workspace-changes";

const text_encoder = new TextEncoder();

export const terminal_tool_recent_output_maximum_bytes = 32 * 1024;
export const terminal_tool_list_maximum_items = 128;

const BoundedTerminalText = (maximum_bytes: number, allow_empty = false) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) =>
			(!allow_empty && value.length === 0) ||
			text_encoder.encode(value).byteLength > maximum_bytes
				? `Expected ${allow_empty ? "at most" : "non-empty text within"} ${maximum_bytes} UTF-8 bytes`
				: undefined,
		),
	);

const TerminalExecutable = BoundedTerminalText(512).check(
	Schema.makeFilter<string>((value) =>
		/[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected an executable without control characters"
			: undefined,
	),
);

const TerminalArgument = BoundedTerminalText(4096, true).check(
	Schema.makeFilter<string>((value) =>
		/[\p{Cc}\p{Cf}]/u.test(value)
			? "Expected an argument without control characters"
			: undefined,
	),
);

const TerminalArguments = Schema.Array(TerminalArgument).check(
	Schema.makeFilter<ReadonlyArray<string>>((arguments_) =>
		arguments_.length > 64 ||
		text_encoder.encode(arguments_.join("\u0000")).byteLength > 16 * 1024
			? "Expected at most 64 arguments within 16384 UTF-8 bytes"
			: undefined,
	),
);

const TerminalDimension = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(20),
	Schema.isLessThanOrEqualTo(500),
);

export const TerminalStartToolArguments = Schema.Struct({
	args: TerminalArguments,
	cols: Schema.optional(TerminalDimension),
	cwd: Schema.optional(WorkspacePath),
	executable: TerminalExecutable,
	rows: Schema.optional(TerminalDimension),
});

export const TerminalIdentifierArguments = Schema.Struct({
	terminal_id: Identifier,
});

export const TerminalWriteToolArguments = Schema.Struct({
	data: BoundedTerminalText(16 * 1024),
	terminal_id: Identifier,
});

export const TerminalReadRecentToolArguments = Schema.Struct({
	max_bytes: Schema.optional(
		Schema.Int.check(
			Schema.isGreaterThanOrEqualTo(1),
			Schema.isLessThanOrEqualTo(terminal_tool_recent_output_maximum_bytes),
		),
	),
	terminal_id: Identifier,
});

export const TerminalToolMetadata = Schema.Struct({
	generation: PositiveInt,
	state: Schema.Literals(["opening", "active", "closed", "failed"]),
	terminal_id: Identifier,
});

export const TerminalListToolResult = Schema.Struct({
	terminals: Schema.Array(TerminalToolMetadata).check(
		Schema.makeFilter<ReadonlyArray<typeof TerminalToolMetadata.Type>>((terminals) =>
			terminals.length <= terminal_tool_list_maximum_items
				? undefined
				: `Expected at most ${terminal_tool_list_maximum_items} terminals`,
		),
	),
	truncated: Schema.Boolean,
});

export const TerminalCommandToolResult = Schema.Struct({
	status: Schema.Literals(["accepted", "duplicate"]),
	terminal: TerminalToolMetadata,
});

export const TerminalReadRecentToolResult = Schema.Struct({
	data: BoundedTerminalText(44 * 1024, true),
	encoding: Schema.Literal("base64"),
	state: Schema.Literals(["available", "unavailable_after_restart"]),
	terminal: TerminalToolMetadata,
	truncated: Schema.Boolean,
});
