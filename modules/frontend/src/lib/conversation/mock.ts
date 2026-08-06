import { Schema } from "effect";

import {
	ConversationSnapshot as ConversationSnapshotSchema,
	type ConversationSnapshot,
} from "@artisan/protocol";

const fixture_created_at = "2026-07-24T00:00:00.000Z";

const FixtureEntity = (
	id: string,
	ordinal: number,
	lifecycle: "active" | "completed" = "completed",
) => ({
	created_at: fixture_created_at,
	id,
	lifecycle,
	ordinal,
	references: [],
	revision: 1,
	source_refs: [{ provider: "fixture", reference: `fixture:${id}` }],
	updated_at: fixture_created_at,
});

const CommandActivity = (command: string) => ({
	detail: command,
	kind: "terminal_activity",
	label: "Terminal",
});

const SearchActivity = (query: string) => ({
	detail: query,
	kind: "search",
	label: "Search",
});

const ToolActivity = (tool_name: string, detail: string) => ({
	detail,
	kind: "tool",
	label: tool_name,
});

const fixture_turns = [
	{
		activities: [
			CommandActivity('rg -n "ConversationItem|ConversationPatch" modules .tests'),
			CommandActivity("Get-Content -Raw modules/protocol/src/conversation.ts"),
			CommandActivity("Get-Content -Raw modules/frontend/src/lib/conversation/store.ts"),
			CommandActivity(
				"Get-Content -Raw modules/frontend/src/routes/components/thread-workspace.svelte",
			),
			SearchActivity("streaming model tool-call event lifecycle"),
			ToolActivity("github/fetch_file", "Fetched the upstream streaming event reference"),
			CommandActivity("git status --short"),
		],
		answer: "The transcript now consumes stable typed entities rather than guessing structure from provider text.",
		commentary: [
			"I’m tracing the conversation from the engine normalizer through persistence and into the renderer before changing the shape.",
			"The store keys entities correctly, but the fixture never exercised real harness events. I’m replacing its invented activities with commands, searches, and tool calls the adapters actually disclose.",
		],
		prompt: "Can you make the thread transcript stable while tools and reasoning stream in?",
		reasoning:
			"I need to inspect the canonical event boundary before deciding whether this is a renderer problem or a protocol problem.",
		work: "Map the engine and transcript pipeline",
	},
	{
		activities: [
			CommandActivity(
				'rg -n "change_set|file_change|work_session" modules/frontend modules/protocol',
			),
			CommandActivity("Get-Content -Raw modules/frontend/src/lib/conversation/mock.ts"),
			CommandActivity("git diff -- modules/frontend/src/lib/conversation"),
			CommandActivity("pnpm exec vitest run .tests/frontend/conversation-store.test.ts"),
			CommandActivity("git diff --check"),
			CommandActivity("git status --short"),
		],
		answer: "Changed files and “Worked for” are explicit lifecycle entities, so their identity survives streaming updates.",
		commentary: [
			"I found the brittle part: changed files were being inferred from nearby text instead of owned by the turn.",
			"The reducer is in place. I’m checking the turn-end change summary and its replay behavior now.",
		],
		prompt: "Make changed files and work sessions first-class instead of parsing headings.",
		reasoning:
			"The durable representation needs stable IDs for work and file changes, with rendering derived from typed relationships rather than headings.",
		work: "Build the canonical conversation reducer",
	},
	{
		activities: [
			CommandActivity('rg -n "patch_gap|duplicate|revision_not_monotonic" modules .tests'),
			CommandActivity("pnpm exec vitest run .tests/protocol/conversation.test.ts"),
			CommandActivity("pnpm exec vitest run .tests/frontend/conversation-store.test.ts"),
			CommandActivity("Get-Content .tests/frontend/conversation-store.test.ts"),
			CommandActivity("git diff --check"),
			CommandActivity("git status --short"),
		],
		answer: "Duplicate patches are idempotent. Sequence gaps and invalid transitions request a clean snapshot.",
		commentary: [
			"I’m testing reconnects, duplicate delivery, and a missing sequence in the middle of a streamed message.",
			"The duplicate path is stable. A sequence gap preserves the last good view and requests an authoritative snapshot.",
		],
		prompt: "What happens if the stream reconnects or sends the same patch twice?",
		reasoning:
			"Replay must be deterministic and duplicates must not mutate state, while gaps must never be silently accepted.",
		work: "Exercise replay and recovery behavior",
	},
	{
		activities: [
			CommandActivity(
				'rg -n "transaction|publish|snapshot" modules/backend/src/conversation',
			),
			CommandActivity("Get-Content -Raw modules/backend/src/conversation/projection-api.ts"),
			CommandActivity("Get-Content -Raw modules/backend/src/conversation/repository.ts"),
			CommandActivity("pnpm exec vitest run .tests/backend/conversation-projection.test.ts"),
			CommandActivity("pnpm exec vitest run .tests/transport/artisan-client.test.ts"),
			CommandActivity("git diff --check"),
		],
		answer: "The durable projection commits before publishing updates, so reconnecting clients always have an authoritative state.",
		commentary: [
			"I’m checking whether the UI can observe an event before its durable projection exists.",
			"The transaction boundary is correct. I’m doing one reconnect pass to prove hydration begins from committed state.",
		],
		prompt: "How do we stop the live UI from racing the database?",
		reasoning:
			"Publication must happen after commit, and replay must begin from one authoritative snapshot watermark.",
		work: "Connect persistence to the patch stream",
	},
	{
		activities: [
			CommandActivity("pnpm --filter @artisan/frontend run build"),
			ToolActivity("browser/open", "Opened the fixture thread in the development app"),
			ToolActivity("browser/screenshot", "Captured the expanded work trace"),
			CommandActivity("git diff -- modules/frontend/src/routes/components"),
			CommandActivity("git diff --check"),
			CommandActivity("git status --short"),
		],
		answer: "This fixture now uses only observable harness activity: commands, searches, tool calls, typed file changes, reasoning summaries, and assistant messages.",
		commentary: [
			"I’ve replaced the synthetic actions with the sequence the adapters can actually observe. I’m checking the long collapsed trace in the production renderer now.",
		],
		prompt: "Make the mock useful for reviewing the sticky composer too.",
		reasoning:
			"The fixture should exercise realistic event interleaving while remaining deterministic enough for renderer tests.",
		work: "Render the deterministic thread fixture",
	},
] as const;

const changed_files = [
	{
		additions: 78,
		deletions: 22,
		path: "modules/frontend/src/lib/conversation/store.ts",
	},
	{
		additions: 32,
		deletions: 6,
		path: "modules/frontend/src/routes/components/thread-workspace.svelte",
	},
	{
		additions: 127,
		deletions: 0,
		path: "modules/frontend/src/routes/components/conversation-changes-card.svelte",
	},
	{
		additions: 5,
		deletions: 1,
		path: ".tests/frontend/conversation-store.test.ts",
	},
] as const;

/** Creates the development-only transcript used to review the thread workspace. */
export const MakeMockConversation = (thread_id: string): ConversationSnapshot => {
	const items = fixture_turns.flatMap((fixture_turn, index) => {
		const turn_id = `mock-turn-${index + 1}`;
		const ordinal = index * 100;
		const active = index === fixture_turns.length - 1;
		const turn_changed_files = index === 1 ? changed_files : [];

		return [
			{
				...FixtureEntity(`mock-user-${index + 1}`, ordinal + 1),
				text: fixture_turn.prompt,
				turn_id,
				type: "user_message",
			},
			{
				...FixtureEntity(
					`mock-work-${index + 1}`,
					ordinal + 2,
					active ? "active" : "completed",
				),
				...(active ? {} : { ended_at: fixture_created_at }),
				started_at: active ? fixture_created_at : "2026-07-23T22:57:56.000Z",
				status: active ? "active" : "completed",
				title: fixture_turn.work,
				turn_id,
				type: "work_session",
			},
			{
				...FixtureEntity(
					`mock-reasoning-${index + 1}`,
					ordinal + 3,
					active ? "active" : "completed",
				),
				text: fixture_turn.reasoning,
				turn_id,
				type: "reasoning_summary",
			},
			...fixture_turn.activities.flatMap((activity, activity_index) => [
				{
					...FixtureEntity(
						`mock-activity-${index + 1}-${activity_index + 1}`,
						ordinal + 4 + activity_index * 2,
					),
					detail: activity.detail,
					kind: activity.kind,
					label: activity.label,
					status: "completed",
					turn_id,
					type: "activity",
				},
				...(fixture_turn.commentary[activity_index] === undefined
					? []
					: [
							{
								...FixtureEntity(
									`mock-commentary-${index + 1}-${activity_index + 1}`,
									ordinal + 5 + activity_index * 2,
								),
								text: fixture_turn.commentary[activity_index],
								phase: "commentary",
								turn_id,
								type: "assistant_message",
							},
						]),
			]),
			{
				...FixtureEntity(`mock-assistant-${index + 1}`, ordinal + 60),
				phase: "final",
				text: fixture_turn.answer,
				turn_id,
				type: "assistant_message",
			},
			...(turn_changed_files.length === 0
				? []
				: [
						{
							...FixtureEntity(`mock-change-set-${index + 1}`, ordinal + 61),
							file_count: turn_changed_files.length,
							file_ids: turn_changed_files.map(
								(_, file_index) => `mock-file-${index + 1}-${file_index + 1}`,
							),
							state: "applied",
							summary: "Updated the canonical conversation renderer",
							turn_id,
							type: "change_set",
						},
						...turn_changed_files.map((file, file_index) => ({
							...FixtureEntity(
								`mock-file-${index + 1}-${file_index + 1}`,
								ordinal + 62 + file_index,
							),
							change_set_id: `mock-change-set-${index + 1}`,
							diff: {
								additions: file.additions,
								deletions: file.deletions,
								kind: "known",
							},
							operation: file_index === 2 ? "created" : "modified",
							path: file.path,
							turn_id,
							type: "file_change",
						})),
					]),
		];
	});

	return Schema.decodeUnknownSync(ConversationSnapshotSchema)({
		conversation_id: `conversation:${thread_id}`,
		items,
		journal_sequence: 48,
		last_patch_sequence: 0,
		schema_version: 1,
		thread_id,
		turns: fixture_turns.map((_, index) => ({
			...FixtureEntity(
				`mock-turn-${index + 1}`,
				index * 100,
				index === fixture_turns.length - 1 ? "active" : "completed",
			),
			...(index === fixture_turns.length - 2
				? {
						updated_at: new Date(Date.parse(fixture_created_at) - 23_000).toISOString(),
					}
				: {}),
			type: "turn",
		})),
		updated_at: fixture_created_at,
	});
};
