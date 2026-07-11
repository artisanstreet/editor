import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { DecodeCommandEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const mentioned_projects = [
	{
		display_name: "Artisan Editor",
		project_id: "project_artisan",
		root_path: "C:/Users/Sander/Desktop/artisan-editor",
	},
];

const thread = {
	activity_version: 1,
	affinity_version: 1,
	created_at: "2026-07-11T08:00:00.000Z",
	last_activity_at: "2026-07-11T08:00:00.000Z",
	live_status: "Queued",
	metadata_version: 1,
	pinned: false,
	project_affinity_scores: [],
	project_locked: false,
	linked_projects: [],
	thread_id: "thread_1",
	title: "Backend foundation",
	title_locked: false,
	title_source: "initial",
	updated_at: "2026-07-11T08:00:00.000Z",
};

function make_input() {
	return {
		protocol_version: 1,
		schema_version: 1,
		kind: "command",
		message_id: "message_1",
		thread_id: "thread_1",
		origin: "frontend",
		sent_at: "2026-07-10T08:00:00.000Z",
		payload: {
			type: "thread.create",
			title: "Backend foundation",
		},
	};
}

describe("protocol codec", () => {
	it("decodes a valid command envelope", async () => {
		const input = make_input();

		const decoded = await Effect.runPromise(DecodeCommandEnvelope(input));

		expect(decoded).toEqual(input);
	});

	it("rejects an unknown command type", async () => {
		const input = {
			...make_input(),
			payload: {
				type: "terminal.launch",
				terminal_id: "terminal_1",
			},
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects empty or whitespace identifiers", async () => {
		const input = {
			...make_input(),
			message_id: " ",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects timestamps outside the wire format", async () => {
		const input = {
			...make_input(),
			sent_at: "not-an-iso-date",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("rejects impossible calendar timestamps", async () => {
		const input = {
			...make_input(),
			sent_at: "2026-99-99T99:99:99Z",
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).rejects.toBeDefined();
	});

	it("roundtrips structured project mentions through message commands", async () => {
		const input = {
			...make_input(),
			payload: {
				type: "thread.send_message",
				engine_id: "engine_1",
				mentioned_projects,
				text: "Use the Artisan repository.",
				working_directory: "C:/Users/Sander/Desktop/artisan-editor",
			},
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).resolves.toEqual(input);
	});

	it("roundtrips resolved project mentions through metadata refinement commands", async () => {
		const input = {
			...make_input(),
			payload: {
				basis_activity_version: 1,
				basis_metadata_version: 1,
				live_status: "Working",
				mentioned_projects,
				type: "thread.metadata.refine",
			},
		};

		await expect(Effect.runPromise(DecodeCommandEnvelope(input))).resolves.toEqual(input);
	});

	it("roundtrips content-free project-affinity event payloads", async () => {
		const events = [
			{
				type: "filesystem.mutation",
				operation: "rename",
				path: "C:/Users/Sander/Desktop/artisan-editor/old.ts",
				destination_path: "C:/Users/Sander/Desktop/artisan-editor/new.ts",
			},
			{
				type: "process.ownership",
				source: "artisan_tool",
				working_directory: "C:/Users/Sander/Desktop/artisan-editor",
			},
			{
				type: "git.workspace.observed",
				branch: "codex/backend-services",
				changed_file_count: 3,
				has_diff: true,
				root_path: "C:/Users/Sander/Desktop/artisan-editor",
				worktree_path: "C:/Users/Sander/Desktop/artisan-editor",
			},
			{
				type: "thread.message_queued",
				message_id: "message_queued",
				mentioned_projects,
				reason: "no_active_run",
				text: "Queue this work.",
				working_directory: "C:/Users/Sander/Desktop/artisan-editor",
			},
			{
				type: "thread.metadata.updated",
				change: "metadata",
				mentioned_projects,
				thread,
			},
		];

		const envelopes = events.map((payload, index) => ({
			causation_id: "command_1",
			correlation_id: "message_1",
			journal_sequence: index + 1,
			kind: "event",
			message_id: `event_${index}`,
			origin: "backend",
			payload,
			protocol_version: 1,
			schema_version: 1,
			sequence: index + 1,
			sent_at: "2026-07-11T08:00:00.000Z",
			stream_id: "thread_1",
			thread_id: "thread_1",
		}));

		const decoded = await Promise.all(
			envelopes.map((envelope) => Effect.runPromise(DecodeOutboundControlEnvelope(envelope))),
		);

		expect(decoded).toEqual(envelopes);
	});

	it.each([
		[
			"a negative changed file count",
			{
				type: "git.workspace.observed",
				changed_file_count: -1,
				has_diff: false,
				root_path: "C:/repo",
				worktree_path: "C:/repo",
			},
		],
		[
			"an invalid mutation operation",
			{
				type: "filesystem.mutation",
				operation: "move",
				path: "C:/repo/file.ts",
			},
		],
		[
			"an excess event property",
			{
				type: "process.ownership",
				source: "engine",
				working_directory: "C:/repo",
				output: "private process output",
			},
		],
	] as const)("rejects %s", async (_label, payload) => {
		const envelope = {
			causation_id: "command_1",
			correlation_id: "message_1",
			journal_sequence: 1,
			kind: "event",
			message_id: "event_1",
			origin: "backend",
			payload,
			protocol_version: 1,
			schema_version: 1,
			sequence: 1,
			sent_at: "2026-07-11T08:00:00.000Z",
			stream_id: "thread_1",
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});
});
