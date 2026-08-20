import { createHash } from "node:crypto";
import { mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Cause, Deferred, Effect, Exit, Fiber, ManagedRuntime, Option } from "effect";
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

const session_digest = (session: string) => createHash("sha256").update(session).digest("hex");

const is_interrupted = (exit: Exit.Exit<unknown, unknown>) =>
	Exit.isFailure(exit) && Cause.hasInterruptsOnly(exit.cause);

const MakeHeldLoader = async () => {
	const release = await Effect.runPromise(Deferred.make<void>());
	const started = await Effect.runPromise(Deferred.make<void>());
	return {
		release,
		started,
		load_sessions: (sessions: ReadonlyArray<readonly [string, number]>) => () =>
			Deferred.succeed(started, undefined).pipe(
				Effect.andThen(Deferred.await(release)),
				Effect.as(sessions),
			),
	};
};

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

	it("keeps a restored session valid at exact expiry and removes it one millisecond later", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-expiry-equality-"));
		const store_path = join(directory, "forge-sessions.json");
		const session = "expiry-equality";
		let current = 10_000;
		await writeFile(
			store_path,
			JSON.stringify({
				sessions: [{ expires_at: current, hash: session_digest(session) }],
				version: 1,
			}),
		);
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				now: () => current,
				session_store_path: store_path,
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			expect(await Effect.runPromise(authority.HasSession(session))).toBe(true);
			current += 1;
			expect(await Effect.runPromise(authority.HasSession(session))).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("constructs immediately while durable loading is held", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-held-load-"));
		const held = await MakeHeldLoader();
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: held.load_sessions([]),
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			await Effect.runPromise(Deferred.await(held.started));
			expect(await Effect.runPromise(authority.RequestPair)).toHaveLength(43);
		} finally {
			await runtime.dispose();
		}
	});

	it("orders a new pairing after load without losing restored sessions", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-load-order-"));
		const held = await MakeHeldLoader();
		const now = 1_000;
		const restored = "restored-cookie";
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: held.load_sessions([[session_digest(restored), now + 10_000]]),
				now: () => now,
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			const code = await Effect.runPromise(authority.RequestPair);
			const waiting = Effect.runFork(authority.ConsumePair(code));
			await Effect.runPromise(Deferred.await(held.started));
			await Effect.runPromise(Deferred.succeed(held.release, undefined));
			const session = Option.getOrThrow(await Effect.runPromise(Fiber.join(waiting)));
			expect(await Effect.runPromise(authority.HasSession(restored))).toBe(true);
			expect(await Effect.runPromise(authority.HasSession(session))).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("normalizes duplicate digests before building and persisting bounded indexes", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-bounded-"));
		const store_path = join(directory, "forge-sessions.json");
		let now = 1_000;
		const unique_sessions = Array.from({ length: 300 }, (_, index) => {
			const session = `retained-${index}`;
			return { expires_at: now + index + 1, hash: session_digest(session) };
		});
		const duplicate = unique_sessions.at(-1);
		if (duplicate === undefined) throw new Error("expected the seeded session set to be full");
		const stored_sessions = [
			...unique_sessions,
			...Array.from({ length: 3_700 }, (_, index) => ({
				expires_at: duplicate.expires_at + index,
				hash: duplicate.hash,
			})),
		];
		await writeFile(store_path, JSON.stringify({ sessions: stored_sessions, version: 1 }));
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				now: () => now,
				session_store_path: store_path,
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			expect(await Effect.runPromise(authority.HasSession("retained-0"))).toBe(false);
			expect(await Effect.runPromise(authority.HasSession("retained-299"))).toBe(true);
			expect(await Effect.runPromise(authority.HasSession("not-in-the-map"))).toBe(false);
			const normalized = JSON.parse(await readFile(store_path, "utf8")) as {
				sessions: Array<{ hash: string }>;
			};
			expect(normalized.sessions).toHaveLength(256);
			expect(new Set(normalized.sessions.map((entry) => entry.hash)).size).toBe(256);
			now += 10_000;
			expect(await Effect.runPromise(authority.HasSession("retained-299"))).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects oversized, over-cardinality, and non-digest stores before publication", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-store-bounds-"));
		const scenarios = [
			"x".repeat(512 * 1_024 + 1),
			JSON.stringify({
				sessions: Array.from({ length: 4_097 }, (_, index) => ({
					expires_at: 10_000 + index,
					hash: session_digest(`cardinality-${index}`),
				})),
				version: 1,
			}),
			JSON.stringify({
				sessions: [{ expires_at: 10_000, hash: "A".repeat(64) }],
				version: 1,
			}),
		];
		for (const [index, content] of scenarios.entries()) {
			const store_path = join(directory, `forge-sessions-${index}.json`);
			await writeFile(store_path, content);
			const runtime = ManagedRuntime.make(
				make_forge_control_authority_layer({
					now: () => 1_000,
					session_store_path: store_path,
				}),
			);
			try {
				const authority = await runtime.runPromise(ForgeControlAuthority);
				expect(await Effect.runPromise(authority.HasSession("untrusted"))).toBe(false);
			} finally {
				await runtime.dispose();
			}
		}
	});

	it("fails open to an empty store when the background load fails", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-load-failure-"));
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: () => Effect.fail(new Error("unreadable store")),
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			expect(await Effect.runPromise(authority.HasSession("missing"))).toBe(false);
			const code = await Effect.runPromise(authority.RequestPair);
			expect(Option.isSome(await Effect.runPromise(authority.ConsumePair(code)))).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});

	it("allows a waiting caller to interrupt without cancelling the shared load", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-interrupt-"));
		const held = await MakeHeldLoader();
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: held.load_sessions([]),
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		try {
			const authority = await runtime.runPromise(ForgeControlAuthority);
			const waiting = Effect.runFork(authority.HasSession("missing"));
			await Effect.runPromise(Deferred.await(held.started));
			await Effect.runPromise(Fiber.interrupt(waiting));
			const interrupted = await Effect.runPromise(Fiber.await(waiting));
			expect(is_interrupted(interrupted)).toBe(true);
			await Effect.runPromise(Deferred.succeed(held.release, undefined));
			expect(await Effect.runPromise(authority.HasSession("missing"))).toBe(false);
		} finally {
			await runtime.dispose();
		}
	});

	it("lets close win after mint without returning an unusable session", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-mint-close-"));
		const persist_started = await Effect.runPromise(Deferred.make<void>());
		const release_persist = await Effect.runPromise(Deferred.make<void>());
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: () => Effect.succeed([]),
				persist_sessions: (_path, sessions) =>
					sessions.size === 0
						? Effect.void
						: Deferred.succeed(persist_started, undefined).pipe(
								Effect.andThen(Deferred.await(release_persist)),
							),
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		const authority = await runtime.runPromise(ForgeControlAuthority);
		const code = await Effect.runPromise(authority.RequestPair);
		const consuming = Effect.runFork(authority.ConsumePair(code));
		await Effect.runPromise(Deferred.await(persist_started));
		await runtime.dispose();
		expect(is_interrupted(await Effect.runPromise(Effect.exit(authority.RequestPair)))).toBe(
			true,
		);
		await Effect.runPromise(Deferred.succeed(release_persist, undefined));
		expect(await Effect.runPromise(Fiber.join(consuming))).toEqual(Option.none());
	});

	it("settles held waiters on scope close and rejects late load publication", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-sessions-scope-close-"));
		const held = await MakeHeldLoader();
		const runtime = ManagedRuntime.make(
			make_forge_control_authority_layer({
				load_sessions: held.load_sessions([[session_digest("late"), 99_999]]),
				session_store_path: join(directory, "forge-sessions.json"),
			}),
		);
		const authority = await runtime.runPromise(ForgeControlAuthority);
		const waiting = Effect.runFork(authority.HasSession("late"));
		await Effect.runPromise(Deferred.await(held.started));
		await runtime.dispose();
		expect(await Effect.runPromise(Fiber.join(waiting))).toBe(false);
		await Effect.runPromise(Deferred.succeed(held.release, undefined));
		expect(await Effect.runPromise(authority.HasSession("late"))).toBe(false);
	});
});
