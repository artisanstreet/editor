import { Schema } from "effect";

import {
	ConversationPatch as ConversationPatchSchema,
	ConversationSnapshot as ConversationSnapshotSchema,
	RebuildConversation,
	type ConversationItem,
	type ConversationLifecycle,
	type ConversationPatch,
	type ConversationSnapshot,
} from "@artisan/protocol";

/**
 * Scripts one conversation as it arrives, patch by patch.
 *
 * Harnesses and models differ less in what they can emit than in what they
 * choose to: a frontier model hides reasoning entirely and answers in one
 * final message, while a small open model narrates every thought. Those shapes
 * exercise completely different renderer paths — trace retirement, commentary
 * folding, "thought" versus "worked" duration — and the ones that break are
 * rarely the ones a live session happens to produce.
 *
 * A script is an initial snapshot plus ordered patches, which is exactly what
 * the transport delivers. A recorder that captures a real session can emit this
 * same shape without the format changing.
 *
 * @since 0.8.0
 */
export interface EmulatorScript {
	/** What this shape is meant to reveal, shown beside the timeline. */
	readonly description: string;
	readonly id: string;
	readonly patches: ReadonlyArray<ConversationPatch>;
	readonly snapshot: ConversationSnapshot;
	readonly title: string;
}

const thread_id = "thread_emulator";
const started_at = "2026-07-30T09:00:00.000Z";

/** Ordinals are unique across turns and items alike, so both draw from one counter. */
const make_ordinals = () => {
	let next = 0;
	return () => {
		next += 1;
		return next;
	};
};

const entity = (id: string, ordinal: number, lifecycle: ConversationLifecycle) => ({
	created_at: started_at,
	id,
	lifecycle,
	ordinal,
	references: [],
	revision: 1,
	source_refs: [{ provider: "emulator", reference: `emulator:${id}` }],
	updated_at: started_at,
});

/**
 * Authors one script, validating both halves against the protocol.
 *
 * Decoding here rather than at render time means a malformed script fails in
 * the module that owns it — and in the test that walks every script — instead
 * of looking like a renderer defect on the page.
 */
const script = (input: {
	readonly description: string;
	readonly id: string;
	readonly items: ReadonlyArray<(ordinal: () => number) => ReadonlyArray<unknown>>;
	readonly title: string;
}): EmulatorScript => {
	const ordinal = make_ordinals();
	const raw_patches = input.items.flatMap((build) => build(ordinal));
	const patches = raw_patches.map((patch, index) =>
		Schema.decodeUnknownSync(ConversationPatchSchema)({
			...(patch as Record<string, unknown>),
			patch_id: `patch_${input.id}_${index + 1}`,
			sequence: index + 1,
		}),
	);

	return {
		description: input.description,
		id: input.id,
		patches,
		snapshot: Schema.decodeUnknownSync(ConversationSnapshotSchema)({
			conversation_id: `conversation:${thread_id}`,
			items: [],
			journal_sequence: 0,
			last_patch_sequence: 0,
			schema_version: 1,
			thread_id,
			turns: [],
			updated_at: started_at,
		}),
		title: input.title,
	};
};

/** Opens a turn, since every item must name one that already exists. */
const turn = (id: string) => (ordinal: () => number) => [
	{ turn: { ...entity(id, ordinal(), "active"), type: "turn" }, type: "turn_upsert" },
];

const item =
	(fields: Record<string, unknown> & { readonly id: string; readonly turn_id: string }) =>
	(ordinal: () => number) => [
		{
			item: { ...entity(fields.id, ordinal(), "completed"), ...fields },
			type: "item_upsert",
		},
	];

/**
 * Streams one item the way a provider does: an empty streaming shell, appended
 * text, then a lifecycle transition. Completed reasoning retires from the trace
 * while live reasoning stays visible, so a script that jumps straight to
 * `completed` never exercises the state the user actually watches.
 */
const streamed =
	(fields: {
		readonly chunks: ReadonlyArray<string>;
		readonly id: string;
		readonly settle?: ConversationLifecycle;
		readonly turn_id: string;
		readonly type: ConversationItem["type"];
	}) =>
	(ordinal: () => number) => {
		const base = {
			...entity(fields.id, ordinal(), "streaming"),
			text: "",
			turn_id: fields.turn_id,
			type: fields.type,
			...(fields.type === "assistant_message" ? { phase: "unspecified" } : {}),
		};

		return [
			{ item: base, type: "item_upsert" },
			...fields.chunks.map((text, index) => ({
				item_id: fields.id,
				revision: index + 2,
				text,
				type: "item_append" as const,
			})),
			{
				item_id: fields.id,
				lifecycle: fields.settle ?? "completed",
				revision: fields.chunks.length + 2,
				type: "item_lifecycle" as const,
			},
		];
	};

const settle_turn =
	(id: string, lifecycle: ConversationLifecycle = "completed") =>
	() => [{ lifecycle, revision: 2, turn_id: id, type: "turn_lifecycle" }];

/**
 * The archetypes. Each is one provider's habit, not one feature: the point is
 * to see the same renderer under the shapes that actually reach it.
 */
export const emulator_scripts: ReadonlyArray<EmulatorScript> = [
	script({
		description:
			"Reasoning is never emitted and the answer lands in one final message. The turn footer has no thinking to report, so it must not claim any.",
		id: "hidden-reasoning",
		items: [
			turn("turn_hidden"),
			item({
				id: "hidden_user",
				text: "Why is the thread list re-rendering on every keystroke?",
				turn_id: "turn_hidden",
				type: "user_message",
			}),
			streamed({
				chunks: [
					"The list subscribes to the composer's draft store, ",
					"so every keystroke invalidates it. Moving the draft to its own store fixes it.",
				],
				id: "hidden_answer",
				turn_id: "turn_hidden",
				type: "assistant_message",
			}),
			settle_turn("turn_hidden"),
		],
		title: "Frontier model — hidden reasoning",
	}),
	script({
		description:
			"Long visible reasoning that streams before any concrete work starts, then retires from the trace once it completes.",
		id: "visible-reasoning",
		items: [
			turn("turn_visible"),
			item({
				id: "visible_user",
				text: "Rename the workspace file service without breaking its callers.",
				turn_id: "turn_visible",
				type: "user_message",
			}),
			streamed({
				chunks: [
					"Let me think about what depends on this service. ",
					"The registry constructs it, the protocol handlers call it, and two tests stub it. ",
					"If I rename the class without touching the registry the layer will still resolve, ",
					"so the safe order is registry first, then handlers, then tests.",
				],
				id: "visible_reasoning",
				turn_id: "turn_visible",
				type: "reasoning_summary",
			}),
			item({
				id: "visible_work",
				started_at,
				status: "completed",
				title: "Renamed the service and its callers",
				turn_id: "turn_visible",
				type: "work_session",
			}),
			item({
				detail: "Renamed the class and updated the registry binding.",
				id: "visible_activity",
				kind: "write",
				label: "Edited workspace-file-service.ts",
				status: "completed",
				turn_id: "turn_visible",
				type: "activity",
			}),
			streamed({
				chunks: ["Renamed it and updated all four call sites. Tests pass."],
				id: "visible_answer",
				turn_id: "turn_visible",
				type: "assistant_message",
			}),
			settle_turn("turn_visible"),
		],
		title: "Open model — visible reasoning",
	}),
	script({
		description:
			"Commentary messages interleaved with tool activity. Commentary folds into the work group; only the final message is promoted.",
		id: "commentary-heavy",
		items: [
			turn("turn_commentary"),
			item({
				id: "commentary_user",
				text: "Find every place we read the git index.",
				turn_id: "turn_commentary",
				type: "user_message",
			}),
			item({
				id: "commentary_first",
				phase: "commentary",
				text: "I'll grep for the index reads first.",
				turn_id: "turn_commentary",
				type: "assistant_message",
			}),
			item({
				detail: "Searched for diff --cached and read-tree.",
				id: "commentary_search",
				kind: "search",
				label: "Searched the backend",
				status: "completed",
				turn_id: "turn_commentary",
				type: "activity",
			}),
			item({
				id: "commentary_second",
				phase: "commentary",
				text: "Two services read it. Checking whether they share a parser.",
				turn_id: "turn_commentary",
				type: "assistant_message",
			}),
			item({
				detail: "Compared the numstat argument builders.",
				id: "commentary_read",
				kind: "read",
				label: "Read both services",
				status: "completed",
				turn_id: "turn_commentary",
				type: "activity",
			}),
			item({
				id: "commentary_final",
				phase: "final",
				text: "Two readers, one parser. The argument builders are duplicated between them.",
				turn_id: "turn_commentary",
				type: "assistant_message",
			}),
			settle_turn("turn_commentary"),
		],
		title: "Commentary-heavy harness",
	}),
	script({
		description:
			"A long tool run with a change set and file changes, so the changes card and the turn footer's file counts are both exercised.",
		id: "tool-heavy",
		items: [
			turn("turn_tools"),
			item({
				id: "tools_user",
				text: "Add the truncation flag to the diff snapshot.",
				turn_id: "turn_tools",
				type: "user_message",
			}),
			item({
				id: "tools_work",
				started_at,
				status: "completed",
				title: "Added the flag and its tests",
				turn_id: "turn_tools",
				type: "work_session",
			}),
			item({
				detail: "pnpm exec vitest run .tests/backend",
				id: "tools_terminal",
				kind: "terminal_activity",
				label: "Ran the backend suite",
				status: "completed",
				turn_id: "turn_tools",
				type: "activity",
			}),
			item({
				file_count: 2,
				file_ids: ["tools_file_schema", "tools_file_service"],
				id: "tools_changes",
				state: "applied",
				summary: "Carried truncation through the diff snapshot",
				turn_id: "turn_tools",
				type: "change_set",
			}),
			item({
				change_set_id: "tools_changes",
				diff: { additions: 14, deletions: 2, kind: "known" },
				id: "tools_file_schema",
				operation: "modified",
				path: "modules/protocol/src/repository.ts",
				turn_id: "turn_tools",
				type: "file_change",
			}),
			item({
				change_set_id: "tools_changes",
				diff: { kind: "unavailable" },
				id: "tools_file_service",
				operation: "modified",
				path: "modules/backend/src/git/repository-service.ts",
				turn_id: "turn_tools",
				type: "file_change",
			}),
			item({
				id: "tools_answer",
				phase: "final",
				text: "The flag travels with the counts now, and a truncated read says so instead of reporting zero.",
				turn_id: "turn_tools",
				type: "assistant_message",
			}),
			settle_turn("turn_tools"),
		],
		title: "Tool-heavy run with changes",
	}),
	script({
		description:
			"An approval requested mid-run and left unanswered, which is the state the composer and the work group have to share.",
		id: "approval-pending",
		items: [
			turn("turn_approval"),
			item({
				id: "approval_user",
				text: "Push the branch.",
				turn_id: "turn_approval",
				type: "user_message",
			}),
			item({
				id: "approval_work",
				started_at,
				status: "active",
				title: "Pushing the branch",
				turn_id: "turn_approval",
				type: "work_session",
			}),
			item({
				id: "approval_request",
				interaction_id: "interaction_push",
				prompt: "Run `git push --force-with-lease origin master`?",
				request: {
					command: "git push --force-with-lease origin master",
					cwd: "C:/Users/sander/Desktop/artisan-editor",
					kind: "command",
				},
				requested_at: started_at,
				state: "requested",
				turn_id: "turn_approval",
				type: "approval",
			}),
		],
		title: "Approval waiting mid-run",
	}),
	script({
		description:
			"A failure with a scheduled retry, then a recovered answer — the path where a failed work group must stay expanded.",
		id: "error-retry",
		items: [
			turn("turn_error"),
			item({
				id: "error_user",
				text: "Read the runtime catalog.",
				turn_id: "turn_error",
				type: "user_message",
			}),
			item({
				id: "error_work",
				ended_at: started_at,
				started_at,
				status: "failed",
				title: "Reading the catalog",
				turn_id: "turn_error",
				type: "work_session",
			}),
			item({
				id: "error_item",
				message: "The engine dropped the connection while streaming.",
				retry: { after_ms: 2_000, attempt: 1, kind: "scheduled", max_attempts: 3 },
				turn_id: "turn_error",
				type: "error",
			}),
			item({
				id: "error_answer",
				phase: "final",
				text: "Reconnected on the second attempt; the catalog has 34 models.",
				turn_id: "turn_error",
				type: "assistant_message",
			}),
			settle_turn("turn_error"),
		],
		title: "Error with scheduled retry",
	}),
	script({
		description:
			"Context compaction plus native provider events, which only appear when diagnostics are enabled in the shader panel.",
		id: "diagnostics",
		items: [
			turn("turn_diagnostics"),
			item({
				id: "diagnostics_user",
				text: "Keep going.",
				turn_id: "turn_diagnostics",
				type: "user_message",
			}),
			item({
				id: "diagnostics_compaction",
				portability: "provider_bound",
				state: "completed",
				summary: "Compacted 42 turns into a provider-bound summary",
				turn_id: "turn_diagnostics",
				type: "compaction",
			}),
			item({
				id: "diagnostics_native_one",
				summary: "response.output_item.added — reasoning",
				turn_id: "turn_diagnostics",
				type: "native_event",
			}),
			item({
				id: "diagnostics_native_two",
				summary: "response.output_item.done — reasoning (encrypted)",
				turn_id: "turn_diagnostics",
				type: "native_event",
			}),
			item({
				id: "diagnostics_native_warning",
				severity: "warning",
				summary: "Provider retried a dropped stream frame",
				turn_id: "turn_diagnostics",
				type: "native_event",
			}),
			item({
				id: "diagnostics_native_error",
				severity: "error",
				summary: "Provider closed the stream before the turn settled",
				turn_id: "turn_diagnostics",
				type: "native_event",
			}),
			item({
				id: "diagnostics_answer",
				phase: "final",
				text: "Continuing from the compacted context.",
				turn_id: "turn_diagnostics",
				type: "assistant_message",
			}),
			settle_turn("turn_diagnostics"),
		],
		title: "Compaction and diagnostics",
	}),
	script({
		description:
			"A plan with mixed entry states and a question awaiting an answer, so both prompt surfaces render together.",
		id: "plan-and-question",
		items: [
			turn("turn_plan"),
			item({
				id: "plan_user",
				text: "Plan the migration before touching anything.",
				turn_id: "turn_plan",
				type: "user_message",
			}),
			item({
				entries: [
					{ id: "plan_one", state: "completed", text: "Inventory the callers" },
					{ id: "plan_two", state: "active", text: "Introduce the shared parser" },
					{ id: "plan_three", state: "pending", text: "Delete the duplicate builders" },
					{ id: "plan_four", state: "skipped", text: "Rename the module" },
				],
				id: "plan_item",
				state: "active",
				turn_id: "turn_plan",
				type: "plan",
			}),
			item({
				id: "plan_question",
				interaction_id: "interaction_scope",
				prompt: "Should the shared parser live in parsers.ts or its own module?",
				requested_at: started_at,
				state: "requested",
				turn_id: "turn_plan",
				type: "question",
			}),
		],
		title: "Plan with an open question",
	}),
];

/**
 * Materializes one step of a script.
 *
 * Every step is a fresh rebuild rather than an incremental step, so scrubbing
 * backwards is the same operation as scrubbing forwards and cannot drift from
 * what a client applying the same patches would hold.
 */
export const EmulatorSnapshotAt = (
	script_input: EmulatorScript,
	step: number,
): ConversationSnapshot | { readonly error: string } => {
	const bounded = Math.max(0, Math.min(script_input.patches.length, Math.trunc(step)));
	const result = RebuildConversation(
		script_input.snapshot,
		script_input.patches.slice(0, bounded),
	);

	if (result._tag === "invariant_error") {
		return { error: `${result.error.code}: ${result.error.message}` };
	}

	return result.state.snapshot;
};

/** Names one step for the timeline, so a position is readable without counting. */
export const EmulatorStepLabel = (script_input: EmulatorScript, step: number): string => {
	if (step === 0) return "Empty conversation";

	const patch = script_input.patches[step - 1];
	if (patch === undefined) return "Complete";
	if (patch.type === "item_upsert") return `${patch.item.type} · ${patch.item.lifecycle}`;
	if (patch.type === "item_append") return `append → ${patch.item_id}`;
	if (patch.type === "item_lifecycle") return `${patch.item_id} → ${patch.lifecycle}`;
	if (patch.type === "turn_upsert") return `turn ${patch.turn.id}`;

	return `turn ${patch.turn_id} → ${patch.lifecycle}`;
};
