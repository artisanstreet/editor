import { mkdtemp, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, ManagedRuntime, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	ForgeControlAuthority,
	make_forge_control_authority_layer,
} from "../../modules/forge/src/index";

const PairSession = Effect.gen(function* () {
	const authority = yield* ForgeControlAuthority;
	const code = yield* authority.RequestPair;
	return Option.getOrThrow(yield* authority.ConsumePair(code));
});

describe("forge session persistence", () => {
	it("keeps a paired session valid across an authority restart", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-"));
		const store_path = join(directory, "forge-sessions.json");

		const first = ManagedRuntime.make(
			make_forge_control_authority_layer({ session_store_path: store_path }),
		);
		const session = await first.runPromise(PairSession);
		expect(
			await first.runPromise(
				Effect.flatMap(ForgeControlAuthority, (authority) => authority.HasSession(session)),
			),
		).toBe(true);
		await first.dispose();

		const second = ManagedRuntime.make(
			make_forge_control_authority_layer({ session_store_path: store_path }),
		);
		try {
			expect(
				await second.runPromise(
					Effect.flatMap(ForgeControlAuthority, (authority) =>
						authority.HasSession(session),
					),
				),
			).toBe(true);
			expect(
				await second.runPromise(
					Effect.flatMap(ForgeControlAuthority, (authority) =>
						authority.HasSession("forged-session-value"),
					),
				),
			).toBe(false);
		} finally {
			await second.dispose();
		}
	});

	it("stores only digests, never the cookie value", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-digest-"));
		const store_path = join(directory, "forge-sessions.json");

		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({ session_store_path: store_path }),
		);
		try {
			const session = await runtime.runPromise(PairSession);
			const stored = await readFile(store_path, "utf8");
			expect(stored).not.toContain(session);
			expect(JSON.parse(stored)).toMatchObject({ version: 1 });
		} finally {
			await runtime.dispose();
		}
	});

	it("drops expired sessions when restoring the store", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-expiry-"));
		const store_path = join(directory, "forge-sessions.json");
		let current = 1_000;

		const first = ManagedRuntime.make(
			make_forge_control_authority_layer({
				now: () => current,
				session_store_path: store_path,
			}),
		);
		const session = await first.runPromise(PairSession);
		await first.dispose();

		current += 13 * 60 * 60 * 1_000;
		const second = ManagedRuntime.make(
			make_forge_control_authority_layer({
				now: () => current,
				session_store_path: store_path,
			}),
		);
		try {
			expect(
				await second.runPromise(
					Effect.flatMap(ForgeControlAuthority, (authority) =>
						authority.HasSession(session),
					),
				),
			).toBe(false);
		} finally {
			await second.dispose();
		}
	});
});
