import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Effect } from "effect";

import type { CommandEnvelope, ContentIdentity } from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolRouter,
	WorkspaceChangeRepository,
	WorkspaceSnapshotStore,
} from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

function content_identity(content: Uint8Array): ContentIdentity {
	return {
		algorithm: "sha256",
		byte_count: content.byteLength,
		content_hash: createHash("sha256").update(content).digest("hex"),
	};
}

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-snapshot-composition-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function create_thread_command(thread_id: string): CommandEnvelope {
	return {
		kind: "command",
		message_id: `create_${thread_id}`,
		origin: "frontend",
		payload: {
			title: "Workspace snapshot composition",
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-12T00:00:00.000Z",
		thread_id,
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("workspace snapshot production composition", () => {
	it("claims and stages a replacement through the complete backend runtime", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({ database_path, migrations_path });
		const thread_id = "thread_snapshot_composition";
		const change_id = "change_snapshot_composition";
		const before = new TextEncoder().encode("before composition");
		const after = new TextEncoder().encode("after composition");

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;
					const repository = yield* WorkspaceChangeRepository;
					const snapshots = yield* WorkspaceSnapshotStore;

					yield* router.Route(create_thread_command(thread_id));
					const claim = yield* repository.ClaimReplace({
						_tag: "replace",
						agent_id: "agent_snapshot_composition",
						change_id,
						expected_before: content_identity(before),
						intended_after: content_identity(after),
						message_id: "message_snapshot_composition",
						path: "src/composition.ts",
						request_fingerprint: "a".repeat(64),
						run_id: "run_snapshot_composition",
						sent_at: "2026-07-12T00:00:01.000Z",
						thread_id,
						workspace_id: "workspace_snapshot_composition",
					});
					const snapshot = yield* snapshots.Stage({
						change_id,
						content: before,
						expected_identity: content_identity(before),
						thread_id,
					});

					return { claim, snapshot };
				}),
			);

			expect(result.claim).toMatchObject({
				_tag: "claimed",
				operation: { action: "replace", change_id, thread_id },
			});
			expect(result.snapshot).toEqual({ status: "staged" });
		} finally {
			await runtime.dispose();
		}
	});
});
