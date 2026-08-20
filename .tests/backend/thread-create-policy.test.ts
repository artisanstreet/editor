import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime } from "@artisan/backend";

import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationCoordinators } from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

const policy = {
	context_window: "1m",
	engine_id: "codex",
	model: "gpt-5.6-sol",
	permission: "supervised",
	permission_mode: "on_request",
	reasoning_effort: "high",
	sandbox_mode: "workspace_write",
	service_tier: "standard",
	strict_clarification: true,
	web_search_enabled: true,
} as const;

const Command = (thread_id: string, policy_input?: typeof policy): CommandEnvelope => ({
	kind: "command",
	message_id: `create_${thread_id}`,
	origin: "frontend",
	payload: {
		...(policy_input === undefined ? {} : { policy: policy_input }),
		title: "Atomic create",
		type: "thread.create",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
	thread_id,
});

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("atomic thread creation policy", () => {
	it("persists the exact initial policy in the create transaction and duplicate creates no second coordinator", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-thread-create-policy-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const sessions = yield* OrchestrationRepository;
					const database = yield* Database;
					const accepted = yield* journal.AcceptThreadCreate(
						Command("thread_atomic", policy),
					);
					const session = yield* sessions.GetSession("thread_atomic");
					const duplicate = yield* journal.AcceptThreadCreate(
						Command("thread_atomic", policy),
					);
					const coordinators = yield* database.client
						.select()
						.from(OrchestrationCoordinators);
					return { accepted, coordinators, duplicate, session };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.session.policy).toEqual(policy);
			expect(result.coordinators).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("keeps the default session policy when no initial policy is supplied", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-thread-create-default-"));
		temporary_directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const sessions = yield* OrchestrationRepository;
					const database = yield* Database;
					yield* journal.AcceptThreadCreate(Command("thread_default"));
					return {
						coordinators: yield* database.client
							.select()
							.from(OrchestrationCoordinators),
						session: yield* sessions.GetSession("thread_default"),
					};
				}),
			);

			expect(result.session.policy).toMatchObject({
				engine_id: "codex",
				permission: "supervised",
				service_tier: "standard",
			});
			expect(result.coordinators).toHaveLength(0);
		} finally {
			await runtime.dispose();
		}
	});
});
