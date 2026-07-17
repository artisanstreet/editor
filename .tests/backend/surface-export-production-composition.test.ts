import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Redacted } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { ExportControlDecision, ExportControlPolicy } from "@artisan/protocol";

import {
	ExportControlGate,
	make_export_control_policy_source_layer,
} from "../../modules/backend/src/compliance/export-control";
import { make_node_export_control_intent_commitment_layer } from "../../modules/backend/src/compliance/node-export-control-intent-commitment";
import { make_backend_runtime } from "../../modules/backend/src/runtime/backend-runtime";
import { SurfaceProjectionRebuilder } from "../../modules/backend/src/surface/surface-projection-rebuilder";
import { SurfaceProjectionStore } from "../../modules/backend/src/surface/surface-projection-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-surface-export-composition-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

const policy: ExportControlPolicy = {
	action_requirements: [{ action: "release", required_signal_kinds: ["account_country"] }],
	denied_country_codes: ["RU"],
	effective_at: "2020-01-01T00:00:00.000Z",
	expires_at: "2030-01-01T00:00:00.000Z",
	legal_review: {
		approved_at: "2020-01-01T00:00:00.000Z",
		expires_at: "2030-01-01T00:00:00.000Z",
		reference: "legal_review_composition",
		status: "approved",
	},
	policy_id: "policy_composition",
	schema_version: 1,
	support_url: "https://artisan.example/support/export-control",
	version: 1,
};

function make_intent_commitment() {
	return make_node_export_control_intent_commitment_layer(
		Redacted.make(new Uint8Array(32).fill(29)),
	);
}

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) =>
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("surface and export-control production composition", () => {
	it("exposes durable rebuild services and defaults protected actions closed", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_backend_runtime({ database_path, migrations_path });

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const gate = yield* ExportControlGate;
					const rebuilder = yield* SurfaceProjectionRebuilder;
					const store = yield* SurfaceProjectionStore;
					const before = yield* store.Read;
					const rebuilt = yield* rebuilder.Rebuild;
					const verified = yield* rebuilder.Verify;
					const decision = yield* gate.Check({
						action: "release",
						decision_id: "decision_default_closed",
						signals: [{ country_code: "NO", kind: "account_country" }],
					});

					return { before, decision, rebuilt, verified };
				}),
			);

			expect(result.before).toEqual({ items: [], stream_cursors: [], watermark: 0 });
			expect(result.rebuilt).toMatchObject({
				items: [
					expect.objectContaining({
						group: "Guidance",
						kind: "guidance",
						surface_id: "surface:guidance:global",
					}),
				],
				stream_cursors: [{ sequence: 1, stream_id: "settings:guidance" }],
				watermark: 1,
			});
			expect(result.verified).toMatchObject({ equivalent: true });
			expect(result.decision).toMatchObject({
				code: "audit_unavailable",
				decision: "unavailable",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("exact-replays the durable denial after restart and applies a current policy to new intent", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const request = {
			action: "release",
			decision_id: "decision_restart_closed",
			signals: [{ country_code: "NO", kind: "account_country" }],
		} as const;
		const first_runtime = make_backend_runtime({
			database_path,
			export_control_intent_commitment: make_intent_commitment(),
			migrations_path,
		});
		let first_decision: ExportControlDecision;

		try {
			first_decision = await first_runtime.runPromise(
				ExportControlGate.pipe(Effect.flatMap((gate) => gate.Check(request))),
			);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			export_control_intent_commitment: make_intent_commitment(),
			export_control_policy_source: make_export_control_policy_source_layer(
				Effect.succeed(policy),
			),
			migrations_path,
		});

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					const gate = yield* ExportControlGate;
					const replay = yield* gate.Check(request);
					const allowed = yield* gate.Check({
						...request,
						decision_id: "decision_current_policy",
					});

					return { allowed, replay };
				}),
			);

			expect(result.replay).toEqual(first_decision);
			expect(result.replay).toMatchObject({
				code: "policy_unavailable",
				decision: "unavailable",
			});
			expect(result.allowed).toMatchObject({
				decision: "allowed",
				policy_id: "policy_composition",
				policy_version: 1,
			});
		} finally {
			await second_runtime.dispose();
		}
	});
});
