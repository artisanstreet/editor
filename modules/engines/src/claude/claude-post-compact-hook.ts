import { promises as fs } from "node:fs";
import { join } from "node:path";

import { NodeCrypto } from "@effect/platform-node-shared";

import { Clock, Crypto, Effect, Schema } from "effect";

const maximum_input_bytes = 320 * 1024;
const maximum_summary_bytes = 128 * 1024;
const maximum_path_length = 4_096;
const maximum_session_length = 256;
const claim_pattern = /^[A-Za-z0-9_-]{43}$/;

const Bounded = (maximum: number) => Schema.NonEmptyString.check(Schema.isMaxLength(maximum));
const HookInput = Schema.Struct({
	compact_summary: Bounded(maximum_summary_bytes),
	cwd: Bounded(maximum_path_length),
	hook_event_name: Schema.Literal("PostCompact"),
	session_id: Bounded(maximum_session_length),
	transcript_path: Bounded(maximum_path_length),
	trigger: Schema.Literals(["manual", "auto"]),
});
const Environment = Schema.Struct({
	artisan_run_id: Bounded(256),
	claim: Bounded(43).check(Schema.isPattern(claim_pattern)),
	mailbox: Bounded(maximum_path_length),
});

const ReadStdin = Effect.tryPromise({
	try: async () => {
		const chunks: Array<Buffer> = [];
		let size = 0;
		for await (const chunk of process.stdin) {
			size += chunk.length;
			if (size > maximum_input_bytes) throw new Error("PostCompact input exceeds bound");
			chunks.push(chunk);
		}
		return Buffer.concat(chunks).toString("utf8");
	},
	catch: () => new Error("invalid PostCompact input"),
});

const Main = Effect.gen(function* () {
	const crypto = yield* Crypto.Crypto;
	const environment = yield* Schema.decodeUnknownEffect(Environment)({
		artisan_run_id: process.env.ARTISAN_CLAUDE_HOOK_RUN_ID,
		claim: process.env.ARTISAN_CLAUDE_HOOK_CLAIM,
		mailbox: process.env.ARTISAN_CLAUDE_HOOK_MAILBOX,
	});
	const raw = yield* ReadStdin;
	const parsed = yield* Effect.try({
		try: () => JSON.parse(raw),
		catch: () => new Error("invalid PostCompact JSON"),
	});
	const hook = yield* Schema.decodeUnknownEffect(HookInput)(parsed);
	if (new TextEncoder().encode(hook.compact_summary).byteLength > maximum_summary_bytes)
		return yield* Effect.fail(new Error("PostCompact summary exceeds byte bound"));
	const stat = yield* Effect.tryPromise({
		try: () => fs.lstat(hook.transcript_path),
		catch: () => new Error("invalid transcript"),
	});
	if (!stat.isFile() || stat.isSymbolicLink())
		return yield* Effect.fail(new Error("invalid transcript"));
	const now = yield* Clock.currentTimeMillis;
	const value = JSON.stringify({
		artisan_run_id: environment.artisan_run_id,
		claim: environment.claim,
		hook,
		received_at: new Date(now).toISOString(),
		schema_version: 1,
		transcript_identity: {
			device: String(stat.dev),
			inode: String(stat.ino),
			size_before_append: stat.size,
		},
	});
	const temporary = join(environment.mailbox, `.${yield* crypto.randomUUIDv4}.tmp`);
	const output = join(environment.mailbox, `capture-${yield* crypto.randomUUIDv4}.json`);
	yield* Effect.tryPromise({
		try: async () => {
			await fs.writeFile(temporary, value, { flag: "wx", mode: 0o600 });
			await fs.rename(temporary, output);
		},
		catch: () => new Error("failed to persist PostCompact capture"),
	}).pipe(
		Effect.ensuring(
			Effect.tryPromise(() => fs.rm(temporary, { force: true })).pipe(Effect.ignore),
		),
	);
});

void Effect.runPromise(
	Main.pipe(
		Effect.provide(NodeCrypto.layer),
		Effect.catch(() => Effect.void),
	),
);
