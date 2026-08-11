import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime } from "@artisan/backend";
import type { SessionModelDefaultsUpdate } from "@artisan/protocol";

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
});
