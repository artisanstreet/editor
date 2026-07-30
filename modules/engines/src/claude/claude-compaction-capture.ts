import { promises as fs } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";

import {
	Context,
	Crypto,
	Effect,
	Encoding,
	FileSystem,
	Layer,
	Option,
	Path,
	Schema,
	Scope,
} from "effect";

import { normalize_engine_compaction_summary, type EngineNativeCompaction } from "../engine";

const max_summary_bytes = 128 * 1024;
const max_mailbox_bytes = 320 * 1024;
const max_tail_bytes = 2 * 1024 * 1024;
const tail_timeout_ms = 5_000;
const max_records = 16;
const max_path_length = 4_096;
const max_session_length = 256;
const base64url_256 = /^[A-Za-z0-9_-]{43}$/;
type ScanOutcome =
	| {
			readonly _tag: "candidate";
			readonly value?: {
				readonly compaction: ClaudeCapturedNativeCompaction;
				readonly transcript_offset: number;
			};
	  }
	| { readonly _tag: "conflict" };

const BoundedString = (maximum: number) => Schema.NonEmptyString.check(Schema.isMaxLength(maximum));

const BoundedSummary = BoundedString(max_summary_bytes);

const HookInput = Schema.Struct({
	compact_summary: BoundedSummary,
	cwd: BoundedString(max_path_length),
	hook_event_name: Schema.Literal("PostCompact"),
	session_id: BoundedString(max_session_length),
	transcript_path: BoundedString(max_path_length),
	trigger: Schema.Literals(["manual", "auto"]),
});

const MailboxRecord = Schema.Struct({
	artisan_run_id: BoundedString(256),
	claim: BoundedString(43).check(Schema.isPattern(base64url_256)),
	hook: HookInput,
	received_at: Schema.DateTimeUtcFromString,
	schema_version: Schema.Literal(1),
	transcript_identity: Schema.Struct({
		device: Schema.NonEmptyString,
		inode: Schema.NonEmptyString,
		size_before_append: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	}),
});

export type ClaudeCompactionMailboxRecord = typeof MailboxRecord.Type;

export type ClaudeCapturedNativeCompaction = EngineNativeCompaction;

export interface ClaudeCompactionLease {
	readonly args: ReadonlyArray<string>;
	readonly environment: NodeJS.ProcessEnv;
	readonly Finalize: Effect.Effect<Option.Option<ClaudeCapturedNativeCompaction>>;
}

export interface ClaudeCompactionCaptureOptions {
	readonly claude_config_dir?: string;
	readonly helper_entry?: string;
	readonly node_executable?: string;
	/** Test-only override; production defaults to the bounded five-second race window. */
	readonly settle_timeout_ms?: number;
	readonly temporary_directory?: string;
}

/** Owns private, per-run Claude PostCompact plugin capture state. */
export class ClaudeCompactionCapture extends Context.Service<
	ClaudeCompactionCapture,
	{
		readonly Prepare: (input: {
			readonly artisan_run_id: string;
			readonly environment: NodeJS.ProcessEnv;
			readonly native_session_id: string;
			readonly working_directory: string;
		}) => Effect.Effect<ClaudeCompactionLease, never, Scope.Scope>;
	}
>()("Artisan/ClaudeCompactionCapture") {}

const plugin_hooks = JSON.stringify({
	hooks: {
		PostCompact: [
			{
				matcher: "manual|auto",
				hooks: [
					{
						command: '"${ARTISAN_CLAUDE_HOOK_NODE}" "${ARTISAN_CLAUDE_HOOK_ENTRY}"',
						type: "command",
					},
				],
			},
		],
	},
});

const ReadTail = async (
	record: ClaudeCompactionMailboxRecord,
	expected: {
		readonly artisan_run_id: string;
		readonly claim: string;
		readonly projects_directory: string;
		readonly session_id: string;
		readonly cwd: string;
		readonly hash_summary: (summary: string) => Promise<string>;
	},
) => {
	/** Effect beta.97 has no no-follow lstat/descriptor identity API; this is the bounded Node trust boundary. */
	if (record.claim !== expected.claim || record.artisan_run_id !== expected.artisan_run_id)
		return undefined;
	if (
		record.hook.session_id !== expected.session_id ||
		resolve(record.hook.cwd) !== resolve(expected.cwd)
	)
		return undefined;
	if (new TextEncoder().encode(record.hook.compact_summary).byteLength > max_summary_bytes)
		return undefined;
	const transcript = resolve(record.hook.transcript_path);
	if (basename(transcript) !== `${expected.session_id}.jsonl`) return undefined;
	const real_transcript = await fs.realpath(transcript);
	const project_relative = relative(expected.projects_directory, real_transcript);
	if (
		project_relative.length === 0 ||
		isAbsolute(project_relative) ||
		project_relative === ".." ||
		project_relative.startsWith(`..${String.fromCharCode(92)}`) ||
		project_relative.startsWith("../")
	)
		return undefined;
	const before = await fs.lstat(transcript);
	if (
		!before.isFile() ||
		before.isSymbolicLink() ||
		String(before.dev) !== record.transcript_identity.device ||
		String(before.ino) !== record.transcript_identity.inode
	)
		return undefined;
	if (
		before.size < record.transcript_identity.size_before_append ||
		before.size - record.transcript_identity.size_before_append > max_tail_bytes
	)
		return undefined;
	const handle = await fs.open(transcript, "r");
	let bytes: Buffer;
	try {
		const descriptor_before = await handle.stat();
		if (
			!descriptor_before.isFile() ||
			String(descriptor_before.dev) !== record.transcript_identity.device ||
			String(descriptor_before.ino) !== record.transcript_identity.inode ||
			descriptor_before.size !== before.size
		)
			return undefined;
		bytes = Buffer.alloc(before.size - record.transcript_identity.size_before_append);
		const read = await handle.read(
			bytes,
			0,
			bytes.length,
			record.transcript_identity.size_before_append,
		);
		if (read.bytesRead !== bytes.length) return undefined;
		const descriptor = await handle.stat();
		const after = await fs.lstat(transcript);
		if (
			!after.isFile() ||
			after.isSymbolicLink() ||
			String(after.dev) !== record.transcript_identity.device ||
			String(after.ino) !== record.transcript_identity.inode ||
			String(descriptor.dev) !== record.transcript_identity.device ||
			String(descriptor.ino) !== record.transcript_identity.inode ||
			descriptor.size !== before.size ||
			after.size !== before.size
		)
			return undefined;
	} finally {
		await handle.close().catch(() => undefined);
	}
	const lines = new TextDecoder().decode(bytes).split("\n");
	let boundary: string | undefined;
	for (const line of lines) {
		if (line.length === 0) continue;
		let value: unknown;
		try {
			value = JSON.parse(line);
		} catch {
			return undefined;
		}
		const item = value as {
			type?: unknown;
			subtype?: unknown;
			uuid?: unknown;
			compactMetadata?: { trigger?: unknown };
			isCompactSummary?: unknown;
			message?: { content?: unknown };
		};
		if (boundary === undefined) {
			if (
				item.type !== "system" ||
				item.subtype !== "compact_boundary" ||
				typeof item.uuid !== "string" ||
				item.compactMetadata?.trigger !== record.hook.trigger
			)
				return undefined;
			boundary = item.uuid;
			continue;
		}
		if (item.type === "system" && item.subtype === "compact_boundary") return undefined;
		if (
			item.type !== "user" ||
			item.isCompactSummary !== true ||
			typeof item.message?.content !== "string"
		)
			return undefined;
		const summary_sha256 = await expected.hash_summary(item.message.content);
		if (summary_sha256 !== (await expected.hash_summary(record.hook.compact_summary)))
			return undefined;
		return {
			boundary_id: boundary,
			summary: record.hook.compact_summary,
			summary_sha256,
		};
	}
	return undefined;
};

export const make_claude_compaction_capture_layer = (
	options: ClaudeCompactionCaptureOptions = {},
) =>
	Layer.effect(
		ClaudeCompactionCapture,
		Effect.gen(function* () {
			const crypto = yield* Crypto.Crypto;
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const HashSummary = (summary: string) =>
				Effect.runPromise(
					crypto
						.digest(
							"SHA-256",
							new TextEncoder().encode(normalize_engine_compaction_summary(summary)),
						)
						.pipe(Effect.map(Encoding.encodeHex)),
				);
			return {
				Prepare: (input: {
					readonly artisan_run_id: string;
					readonly environment: NodeJS.ProcessEnv;
					readonly native_session_id: string;
					readonly working_directory: string;
				}): Effect.Effect<ClaudeCompactionLease, never, Scope.Scope> =>
					Effect.acquireRelease(
						Effect.gen(function* () {
							const root = yield* file_system.makeTempDirectory({
								directory: options.temporary_directory ?? tmpdir(),
								prefix: "artisan-claude-compact-",
							});
							const plugin = path_service.join(root, "plugin");
							const mailbox = path_service.join(root, "mailbox");
							yield* file_system.makeDirectory(
								path_service.join(plugin, ".claude-plugin"),
								{ recursive: true },
							);
							yield* file_system.makeDirectory(path_service.join(plugin, "hooks"), {
								recursive: true,
							});
							yield* file_system.makeDirectory(mailbox, { recursive: true });
							yield* file_system.writeFileString(
								path_service.join(plugin, ".claude-plugin", "plugin.json"),
								JSON.stringify({
									name: "artisan-compaction-capture",
									version: "1",
								}),
							);
							yield* file_system.writeFileString(
								path_service.join(plugin, "hooks", "hooks.json"),
								plugin_hooks,
							);
							return { mailbox, plugin, root };
						}),
						({ root }) =>
							file_system
								.remove(root, { recursive: true, force: true })
								.pipe(Effect.ignore),
					).pipe(
						Effect.flatMap(({ mailbox, plugin }) =>
							crypto.randomBytes(32).pipe(
								Effect.map((bytes) => {
									const claim = Encoding.encodeBase64Url(bytes);
									const projects_directory = resolve(
										options.claude_config_dir ??
											input.environment.CLAUDE_CONFIG_DIR ??
											join(homedir(), ".claude"),
										"projects",
									);
									const claimed_boundaries = new Map<string, string>();
									const claimed_offsets = new Map<number, string>();
									const ScanMailbox = Effect.tryPromise({
										try: async () => {
											let candidate:
												| {
														readonly compaction: ClaudeCapturedNativeCompaction;
														readonly transcript_offset: number;
												  }
												| undefined;
											const names = (
												await fs
													.readdir(mailbox)
													.catch(() => [] as Array<string>)
											)
												.filter((name) => name.endsWith(".json"))
												.sort();
											if (names.length > max_records)
												return {
													_tag: "conflict",
												} satisfies ScanOutcome;
											for (const name of names) {
												const mailbox_file = join(mailbox, name);
												const before = await fs
													.lstat(mailbox_file)
													.catch(() => undefined);
												if (
													before === undefined ||
													!before.isFile() ||
													before.isSymbolicLink() ||
													before.size > max_mailbox_bytes
												)
													continue;
												const handle = await fs
													.open(mailbox_file, "r")
													.catch(() => undefined);
												if (handle === undefined) continue;
												let raw: string | undefined;
												try {
													const descriptor = await handle.stat();
													if (
														!descriptor.isFile() ||
														String(descriptor.dev) !==
															String(before.dev) ||
														String(descriptor.ino) !==
															String(before.ino) ||
														descriptor.size !== before.size
													)
														continue;
													const bytes = Buffer.alloc(before.size);
													if (
														(
															await handle.read(
																bytes,
																0,
																bytes.length,
																0,
															)
														).bytesRead !== bytes.length
													)
														continue;
													const after = await fs.lstat(mailbox_file);
													if (
														!after.isFile() ||
														after.isSymbolicLink() ||
														String(after.dev) !== String(before.dev) ||
														String(after.ino) !== String(before.ino) ||
														after.size !== before.size
													)
														continue;
													raw = bytes.toString("utf8");
												} finally {
													await handle.close().catch(() => undefined);
												}
												if (raw === undefined) continue;
												let parsed: unknown;
												try {
													parsed =
														raw.length === 0
															? undefined
															: JSON.parse(raw);
												} catch {
													continue;
												}
												const decoded =
													parsed === undefined
														? Option.none()
														: Schema.decodeUnknownOption(MailboxRecord)(
																parsed,
															);
												if (Option.isNone(decoded)) continue;
												const projects_realpath = await fs
													.realpath(projects_directory)
													.catch(() => undefined);
												if (projects_realpath === undefined) continue;
												const pair = await ReadTail(decoded.value, {
													artisan_run_id: input.artisan_run_id,
													claim,
													cwd: input.working_directory,
													hash_summary: HashSummary,
													projects_directory: projects_realpath,
													session_id: input.native_session_id,
												});
												if (pair !== undefined) {
													const prior = claimed_boundaries.get(
														pair.boundary_id,
													);
													const hash = pair.summary_sha256;
													if (prior !== undefined && prior !== hash)
														return {
															_tag: "conflict",
														} satisfies ScanOutcome;
													claimed_boundaries.set(pair.boundary_id, hash);
													const transcript_offset =
														decoded.value.transcript_identity
															.size_before_append;
													const offset_claim = `${pair.boundary_id}:${hash}`;
													const prior_offset =
														claimed_offsets.get(transcript_offset);
													if (
														prior_offset !== undefined &&
														prior_offset !== offset_claim
													)
														return {
															_tag: "conflict",
														} satisfies ScanOutcome;
													claimed_offsets.set(
														transcript_offset,
														offset_claim,
													);
													if (
														candidate === undefined ||
														transcript_offset >
															candidate.transcript_offset
													)
														candidate = {
															compaction: {
																boundary_id: pair.boundary_id,
																method: "claude_post_compact" as const,
																observation_id: `${input.artisan_run_id}:claude:compact:${pair.boundary_id}`,
																source_native_thread_id:
																	input.native_session_id,
																summary: pair.summary,
																summary_sha256: hash,
																trigger: decoded.value.hook.trigger,
															},
															transcript_offset,
														};
												}
											}
											return {
												_tag: "candidate",
												...(candidate === undefined
													? {}
													: { value: candidate }),
											} satisfies ScanOutcome;
										},
										catch: () => new Error("PostCompact mailbox scan failed"),
									});
									const Finalize = Effect.gen(function* () {
										let candidate:
											| {
													readonly compaction: ClaudeCapturedNativeCompaction;
													readonly transcript_offset: number;
											  }
											| undefined;
										const attempts = Math.max(
											1,
											Math.ceil(
												(options.settle_timeout_ms ?? tail_timeout_ms) / 25,
											),
										);
										for (let attempt = 0; attempt < attempts; attempt += 1) {
											const outcome = yield* ScanMailbox;
											if (outcome._tag === "conflict")
												return Option.none<ClaudeCapturedNativeCompaction>();
											if (
												outcome.value !== undefined &&
												(candidate === undefined ||
													outcome.value.transcript_offset >
														candidate.transcript_offset)
											)
												candidate = outcome.value;
											if (attempt + 1 < attempts)
												yield* Effect.sleep("25 millis");
										}
										return candidate === undefined
											? Option.none<ClaudeCapturedNativeCompaction>()
											: Option.some(candidate.compaction);
									}).pipe(
										Effect.catch(() =>
											Effect.succeed(
												Option.none<ClaudeCapturedNativeCompaction>(),
											),
										),
									);
									const lease: ClaudeCompactionLease = {
										args: ["--plugin-dir", plugin],
										environment: {
											...input.environment,
											ARTISAN_CLAUDE_HOOK_CLAIM: claim,
											ARTISAN_CLAUDE_HOOK_ENTRY:
												options.helper_entry ??
												fileURLToPath(
													new URL(
														"./claude-post-compact-hook.js",
														import.meta.url,
													),
												),
											ARTISAN_CLAUDE_HOOK_MAILBOX: mailbox,
											ARTISAN_CLAUDE_HOOK_NODE:
												options.node_executable ?? process.execPath,
											ARTISAN_CLAUDE_HOOK_RUN_ID: input.artisan_run_id,
										},
										Finalize: Finalize as Effect.Effect<
											Option.Option<ClaudeCapturedNativeCompaction>
										>,
									};
									return lease;
								}),
							),
						),
						Effect.map((lease): ClaudeCompactionLease => lease),
						Effect.catch(() =>
							Effect.succeed<ClaudeCompactionLease>({
								args: [] as ReadonlyArray<string>,
								environment: input.environment,
								Finalize: Effect.succeed(
									Option.none<ClaudeCapturedNativeCompaction>(),
								),
							}),
						),
					),
			};
		}).pipe(
			Effect.provide(NodeFileSystem.layer),
			Effect.provide(NodePath.layer),
			Effect.provide(NodeCrypto.layer),
		),
	);
