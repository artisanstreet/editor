import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { SessionDefaultsUpdateInput, SessionModelDefaultsUpdate } from "@artisan/protocol";

import {
	SessionDefaultsService,
	session_defaults_thread_id,
} from "../../modules/backend/src/settings/session-defaults-service";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const make_path = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-session-defaults-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("session defaults service", () => {
	it("defaults onboarding to incomplete and persists completion", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_path(),
			migrations_path,
		});
		try {
			const readings = await runtime.runPromise(
				Effect.gen(function* () {
					const settings = yield* SessionDefaultsService;
					const initial = yield* settings.Read;
					yield* settings.Update({
						kind: "command",
						message_id: "onboarding-complete",
						origin: "frontend",
						payload: {
							onboarding_completed: true,
							type: "session.defaults.update",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-23T13:00:00.000Z",
						thread_id: session_defaults_thread_id,
					});
					return { completed: yield* settings.Read, initial };
				}),
			);

			expect(readings.initial.onboarding_completed).toBe(false);
			expect(readings.completed.onboarding_completed).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists each model's speed tier independently from its reasoning effort", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_path(),
			migrations_path,
		});
		try {
			const defaults = await runtime.runPromise(
				Effect.gen(function* () {
					const settings = yield* SessionDefaultsService;
					const Update = (message_id: string, model: SessionModelDefaultsUpdate) =>
						settings.Update({
							kind: "command",
							message_id,
							origin: "frontend",
							payload: {
								last_model_id: "codex-sol",
								model,
								type: "session.defaults.update",
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-11T12:00:00.000Z",
							thread_id: session_defaults_thread_id,
						});

					yield* Update("session-defaults-fast", {
						model_id: "codex-sol",
						reasoning_effort: "xhigh",
						service_tier: "fast",
					});
					yield* Update("session-defaults-effort", {
						model_id: "codex-sol",
						reasoning_effort: "high",
					});
					return yield* settings.Read;
				}),
			);

			expect(defaults.last_model_id).toBe("codex-sol");
			expect(defaults.models).toEqual([
				{ model_id: "codex-sol", reasoning_effort: "high", service_tier: "fast" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("defaults thread titling to summaries and persists an explicit mode without disturbing other patches", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_path(),
			migrations_path,
		});
		try {
			const readings = await runtime.runPromise(
				Effect.gen(function* () {
					const settings = yield* SessionDefaultsService;
					const Update = (message_id: string, payload: SessionDefaultsUpdateInput) =>
						settings.Update({
							kind: "command",
							message_id,
							origin: "frontend",
							payload: { type: "session.defaults.update", ...payload },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-20T12:00:00.000Z",
							thread_id: session_defaults_thread_id,
						});

					const initial = yield* settings.Read;
					yield* Update("title-mode-latest", { thread_title_mode: "latest_message" });
					const explicit = yield* settings.Read;
					yield* Update("title-mode-unrelated", { auto_continue_usage_limits: false });
					const untouched = yield* settings.Read;
					return { explicit, initial, untouched };
				}),
			);

			expect(readings.initial.thread_title_mode).toBe("summary");
			expect(readings.explicit.thread_title_mode).toBe("latest_message");
			expect(readings.untouched.thread_title_mode).toBe("latest_message");
			expect(readings.untouched.auto_continue_usage_limits).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("rewrites retired permission defaults as Auto", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_path(),
			migrations_path,
		});
		try {
			const permission = await runtime.runPromise(
				Effect.gen(function* () {
					const settings = yield* SessionDefaultsService;
					yield* settings.Update({
						kind: "command",
						message_id: "retired-permission",
						origin: "frontend",
						payload: { permission: "trusted", type: "session.defaults.update" },
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-21T12:00:00.000Z",
						thread_id: session_defaults_thread_id,
					});
					return (yield* settings.Read).permission;
				}),
			);

			expect(permission).toBe("autonomous");
		} finally {
			await runtime.dispose();
		}
	});
});
