import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Crypto, Effect, FileSystem, Layer, Option, Path, PlatformError, Schema } from "effect";

import {
	GitLocalBranchName,
	GitObjectId,
	GitRemoteName,
	type WorkspaceGitMutationRejectionReason,
} from "@artisan/protocol";

import {
	GitMutation,
	GitMutationAttempt,
	GitMutationError,
	GitMutationPlan,
	GitMutationPreparation,
	GitRemoteEndpoint,
	type GitMutationActionAnchor,
	type GitMutationPlan as GitMutationPlanType,
	type GitMutationReconciliation,
	type GitMutationSourceProof,
} from "./git-mutation";
import {
	make_node_process_runner_layer,
	type NodeProcessRunnerOptions,
} from "./node-process-runner";
import { ProcessRunner, type ProcessRunnerResult, type ProcessRunnerShape } from "./process-runner";

/** Configures bounded local Git mutation capability for one already-selected workspace. */
export interface NodeGitMutationOptions {
	readonly cwd: string;
	readonly git_executable?: string;
	readonly max_stderr_bytes?: number;
	readonly max_stdout_bytes?: number;
	readonly process?: NodeProcessRunnerOptions;
}

interface RepositoryPin {
	readonly environment: Readonly<Record<string, string>>;
	readonly git_executable: string;
	readonly git_executable_device: number;
	readonly git_executable_inode?: number;
	readonly git_directory: string;
	readonly git_directory_device: number;
	readonly git_directory_inode?: number;
	readonly identity: string;
	readonly index_path: string;
	readonly object_directory: string;
	readonly object_directory_device: number;
	readonly object_directory_inode?: number;
	readonly object_format: "sha1" | "sha256";
	readonly root: string;
	readonly root_device: number;
	readonly root_inode?: number;
}

interface GitStateObservation {
	readonly merge_head?: string;
	readonly original_head?: string;
	readonly rebase_head?: string;
	readonly state: "merge" | "none" | "rebase";
}

type RefTransactionCommand =
	| { readonly oid: string; readonly ref: string; readonly type: "create" }
	| {
			readonly new_oid: string;
			readonly old_oid: string;
			readonly ref: string;
			readonly type: "update";
	  }
	| { readonly oid: string; readonly ref: string; readonly type: "verify" }
	| {
			readonly old: { readonly type: "oid" | "ref"; readonly value: string };
			readonly ref: string;
			readonly target: string;
			readonly type: "symref_update";
	  };

const decoder = new TextDecoder();
const encoder = new TextEncoder();
const null_device_path = process.platform === "win32" ? "NUL" : "/dev/null";
const inherited_environment_keys = [
	"APPDATA",
	"COMSPEC",
	"HOME",
	"HOMEDRIVE",
	"HOMEPATH",
	"LANG",
	"LC_ALL",
	"LC_CTYPE",
	"LOCALAPPDATA",
	"PATH",
	"PATHEXT",
	"PROGRAMDATA",
	"PROGRAMFILES",
	"PROGRAMFILES(X86)",
	"SSH_AUTH_SOCK",
	"SSL_CERT_DIR",
	"SSL_CERT_FILE",
	"SYSTEMDRIVE",
	"SYSTEMROOT",
	"TEMP",
	"TMP",
	"TMPDIR",
	"USERPROFILE",
	"WINDIR",
	"XDG_CONFIG_HOME",
] as const;

function mutation_error(operation: GitMutationError["operation"], cause?: unknown) {
	return new GitMutationError({ ...(cause === undefined ? {} : { cause }), operation });
}

function is_valid_limit(value: number) {
	return Number.isSafeInteger(value) && value > 0;
}

function is_platform_reason(cause: unknown, reason: PlatformError.SystemErrorTag) {
	return cause instanceof PlatformError.PlatformError && cause.reason._tag === reason;
}

function is_object_id(value: string) {
	return /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/u.test(value);
}

function same_path(left: string, right: string) {
	return process.platform === "win32"
		? left.toLocaleLowerCase("en-US") === right.toLocaleLowerCase("en-US")
		: left === right;
}

function path_is_within(path_service: Path.Path, root: string, candidate: string) {
	const relative = path_service.relative(root, candidate);

	return (
		relative === "" ||
		(relative !== ".." &&
			!relative.startsWith(`..${path_service.sep}`) &&
			!path_service.isAbsolute(relative))
	);
}

function same_bytes(left: Uint8Array, right: Uint8Array) {
	return (
		left.byteLength === right.byteLength && left.every((byte, index) => byte === right[index])
	);
}

function decode_utf8(bytes: Uint8Array) {
	const value = decoder.decode(bytes);

	return same_bytes(encoder.encode(value), bytes) ? value : undefined;
}

function clean_text(result: ProcessRunnerResult) {
	const value = decode_utf8(result.stdout);

	return value?.replace(/\r?\n$/u, "");
}

function split_nul(value: Uint8Array) {
	const decoded = decode_utf8(value);

	if (decoded === undefined) {
		return undefined;
	}

	const values = decoded.split("\0");

	return values.at(-1) === "" ? values.slice(0, -1) : undefined;
}

function has_control_separator(value: string) {
	return /[\r\n]/u.test(value) || value.includes("\0");
}

function is_false_config_value(value: string) {
	return ["", "0", "false", "no", "off"].includes(value.trim().toLocaleLowerCase("en-US"));
}

function is_unsafe_local_config(key: string, value: string) {
	const normalized = key.toLocaleLowerCase("en-US");
	const exact = new Set([
		"core.askpass",
		"core.editor",
		"core.gitproxy",
		"core.hookspath",
		"core.pager",
		"core.sshcommand",
		"diff.external",
		"gpg.program",
		"http.proxy",
		"interactive.difffilter",
		"rebase.instructionformat",
		"sequence.editor",
		"web.browser",
	]);

	if (normalized === "core.fsmonitor") {
		return !is_false_config_value(value);
	}

	return (
		exact.has(normalized) ||
		normalized === "include.path" ||
		normalized.startsWith("includeif.") ||
		normalized.startsWith("alias.") ||
		normalized.startsWith("credential.") ||
		normalized.startsWith("filter.") ||
		normalized.startsWith("http.") ||
		normalized.startsWith("pager.") ||
		normalized.startsWith("protocol.") ||
		/^browser\..*\.cmd$/u.test(normalized) ||
		/^diff\..*\.(?:command|textconv)$/u.test(normalized) ||
		/^difftool\..*\.cmd$/u.test(normalized) ||
		/^gpg\..*\.program$/u.test(normalized) ||
		/^man\..*\.cmd$/u.test(normalized) ||
		/^merge\..*\.driver$/u.test(normalized) ||
		/^mergetool\..*\.cmd$/u.test(normalized) ||
		/^remote\..*\.(?:proxy|receivepack|uploadpack|vcs)$/u.test(normalized) ||
		/^submodule\..*\.update$/u.test(normalized) ||
		/^url\..*\.(?:insteadof|pushinsteadof)$/u.test(normalized)
	);
}

function canonical(value: unknown): string {
	if (value === null) {
		return "null";
	}

	if (typeof value === "string" || typeof value === "boolean") {
		return JSON.stringify(value);
	}

	if (typeof value === "number" && Number.isFinite(value)) {
		return JSON.stringify(value);
	}

	if (Array.isArray(value)) {
		return `[${value.map(canonical).join(",")}]`;
	}

	if (typeof value === "object") {
		const record = value as Record<string, unknown>;
		const entries = Object.keys(record)
			.sort()
			.map((key) => `${JSON.stringify(key)}:${canonical(record[key])}`);

		return `{${entries.join(",")}}`;
	}

	throw new TypeError("Expected a JSON-safe integrity value");
}

function selected_environment() {
	const environment: Record<string, string> = {};

	for (const key of inherited_environment_keys) {
		const value = process.env[key];

		if (value !== undefined) {
			environment[key] = value;
		}
	}

	return {
		...environment,
		GCM_INTERACTIVE: "Never",
		GIT_ATTR_NOSYSTEM: "1",
		GIT_CONFIG_COUNT: "0",
		GIT_CONFIG_GLOBAL: null_device_path,
		GIT_CONFIG_NOSYSTEM: "1",
		GIT_CONFIG_SYSTEM: null_device_path,
		GIT_EDITOR: "true",
		GIT_MERGE_AUTOEDIT: "no",
		GIT_PAGER: "cat",
		GIT_SEQUENCE_EDITOR: "true",
		GIT_TERMINAL_PROMPT: "0",
		NoDefaultCurrentDirectoryInExePath: "1",
		PAGER: "cat",
		SSH_ASKPASS_REQUIRE: "never",
	};
}

function combine_output(
	first: Uint8Array,
	first_bytes: number,
	first_truncated: boolean,
	second: Uint8Array,
	second_bytes: number,
	second_truncated: boolean,
	limit: number,
) {
	const total_bytes = first_bytes + second_bytes;
	const retained = first_truncated
		? first.subarray(0, limit)
		: Buffer.concat([first, second.subarray(0, Math.max(0, limit - first.byteLength))]);

	return {
		bytes: retained,
		total_bytes,
		truncated: first_truncated || second_truncated || total_bytes > retained.byteLength,
	};
}

function combine_results(
	first: ProcessRunnerResult,
	second: ProcessRunnerResult,
	limits: { readonly max_stderr_bytes: number; readonly max_stdout_bytes: number },
) {
	const stderr = combine_output(
		first.stderr,
		first.stderr_bytes,
		first.stderr_truncated,
		second.stderr,
		second.stderr_bytes,
		second.stderr_truncated,
		limits.max_stderr_bytes,
	);
	const stdout = combine_output(
		first.stdout,
		first.stdout_bytes,
		first.stdout_truncated,
		second.stdout,
		second.stdout_bytes,
		second.stdout_truncated,
		limits.max_stdout_bytes,
	);

	return {
		exit_code: second.exit_code,
		stderr: stderr.bytes,
		stderr_bytes: stderr.total_bytes,
		stderr_truncated: stderr.truncated,
		stdout: stdout.bytes,
		stdout_bytes: stdout.total_bytes,
		stdout_truncated: stdout.truncated,
	} satisfies ProcessRunnerResult;
}

function same_source(left: GitMutationSourceProof, right: GitMutationSourceProof) {
	return canonical(left) === canonical(right);
}

function same_pinned_file(device: number, inode: number | undefined, info: FileSystem.File.Info) {
	const observed_inode = Option.getOrUndefined(info.ino);

	return (
		info.type === "Directory" &&
		info.dev === device &&
		(inode === undefined || observed_inode === inode)
	);
}

function transaction_bytes(commands: ReadonlyArray<RefTransactionCommand>) {
	const fields: Array<string> = ["start"];

	for (const command of commands) {
		fields.push("option no-deref");

		if (command.type === "create") {
			fields.push(`create ${command.ref}`, command.oid);
			continue;
		}

		if (command.type === "update") {
			fields.push(`update ${command.ref}`, command.new_oid, command.old_oid);
			continue;
		}

		if (command.type === "verify") {
			fields.push(`verify ${command.ref}`, command.oid);
			continue;
		}

		fields.push(
			`symref-update ${command.ref}`,
			command.target,
			command.old.type,
			command.old.value,
		);
	}

	fields.push("prepare", "commit");

	if (fields.some((field) => field.includes("\0"))) {
		throw new TypeError("Git reference transaction fields must not contain NUL");
	}

	return encoder.encode(`${fields.join("\0")}\0`);
}

function HashJson(crypto: Crypto.Crypto, value: unknown) {
	return Effect.try({
		try: () => encoder.encode(canonical(value)),
		catch: (cause) => mutation_error("integrity", cause),
	}).pipe(
		Effect.flatMap((bytes) => crypto.digest("SHA-256", bytes)),
		Effect.map((digest) => Buffer.from(digest).toString("hex")),
		Effect.mapError((cause) =>
			cause instanceof GitMutationError ? cause : mutation_error("integrity", cause),
		),
	);
}

function HashChunks(crypto: Crypto.Crypto, chunks: ReadonlyArray<Uint8Array>) {
	return Effect.gen(function* () {
		const total_bytes = chunks.reduce((total, chunk) => total + 4 + chunk.byteLength, 4);
		const framed = new Uint8Array(total_bytes);
		const view = new DataView(framed.buffer);
		let offset = 0;

		view.setUint32(offset, chunks.length);
		offset += 4;

		for (const chunk of chunks) {
			view.setUint32(offset, chunk.byteLength);
			offset += 4;
			framed.set(chunk, offset);
			offset += chunk.byteLength;
		}

		const digest = yield* crypto.digest("SHA-256", framed);

		return Buffer.from(digest).toString("hex");
	}).pipe(Effect.mapError((cause) => mutation_error("integrity", cause)));
}

function ResolveGitExecutable(
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	root: string,
	configured?: string,
) {
	return Effect.gen(function* () {
		if (configured !== undefined && !path_service.isAbsolute(configured)) {
			return yield* Effect.fail(mutation_error("configuration"));
		}

		const path_entries = (process.env.PATH ?? "")
			.split(process.platform === "win32" ? ";" : ":")
			.map((entry) => entry.replace(/^"|"$/gu, ""))
			.filter((entry) => entry.length > 0 && path_service.isAbsolute(entry));
		const names = process.platform === "win32" ? ["git.exe", "git.com"] : ["git"];
		const candidates =
			configured === undefined
				? path_entries.flatMap((entry) =>
						names.map((name) => path_service.join(entry, name)),
					)
				: [configured];
		const inspected = yield* Effect.forEach(candidates, (candidate) =>
			Effect.gen(function* () {
				const canonical = yield* file_system.realPath(candidate);
				const info = yield* file_system.stat(canonical);
				const executable = process.platform === "win32" || (info.mode & 0o111) !== 0;

				return info.type === "File" &&
					executable &&
					!path_is_within(path_service, root, canonical)
					? Option.some({ canonical, info })
					: Option.none<{ canonical: string; info: FileSystem.File.Info }>();
			}).pipe(Effect.catch(() => Effect.succeed(Option.none()))),
		);
		const selected = inspected.find(Option.isSome);

		return selected !== undefined
			? selected.value
			: yield* Effect.fail(mutation_error("configuration"));
	}).pipe(
		Effect.mapError((cause) =>
			cause instanceof GitMutationError ? cause : mutation_error("configuration", cause),
		),
	);
}

function BuildRepositoryPin(
	runner: ProcessRunnerShape,
	crypto: Crypto.Crypto,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	options: NodeGitMutationOptions,
) {
	return Effect.gen(function* () {
		const environment = selected_environment();
		const root = yield* file_system.realPath(options.cwd);
		const root_info = yield* file_system.stat(root);
		const executable = yield* ResolveGitExecutable(
			file_system,
			path_service,
			root,
			options.git_executable,
		);
		const RunDiscovery = (args: ReadonlyArray<string>) =>
			runner
				.Run({
					args,
					command: executable.canonical,
					cwd: root,
					environment,
					environment_mode: "replace",
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
				})
				.pipe(
					Effect.mapError((cause) => mutation_error("configuration", cause)),
					Effect.flatMap((result) => {
						const value = clean_text(result);

						return result.exit_code === 0 &&
							!result.stdout_truncated &&
							!result.stderr_truncated &&
							value !== undefined &&
							value.length > 0 &&
							!value.includes("\0")
							? Effect.succeed(value)
							: Effect.fail(mutation_error("configuration"));
					}),
				);

		if (root_info.type !== "Directory") {
			return yield* Effect.fail(mutation_error("configuration"));
		}

		const reported_root = yield* RunDiscovery(["rev-parse", "--show-toplevel"]);
		const canonical_reported_root = yield* file_system.realPath(reported_root);

		if (!same_path(root, canonical_reported_root)) {
			return yield* Effect.fail(mutation_error("configuration"));
		}

		const reported_git_directory = yield* RunDiscovery(["rev-parse", "--absolute-git-dir"]);
		const git_directory = yield* file_system.realPath(reported_git_directory);
		const git_directory_info = yield* file_system.stat(git_directory);

		if (git_directory_info.type !== "Directory") {
			return yield* Effect.fail(mutation_error("configuration"));
		}

		const reported_index_path = yield* RunDiscovery([
			"rev-parse",
			"--path-format=absolute",
			"--git-path",
			"index",
		]);
		const index_path = path_service.normalize(
			path_service.isAbsolute(reported_index_path)
				? reported_index_path
				: path_service.resolve(root, reported_index_path),
		);
		const reported_object_directory = yield* RunDiscovery([
			"rev-parse",
			"--path-format=absolute",
			"--git-path",
			"objects",
		]);
		const object_directory = yield* file_system.realPath(reported_object_directory);
		const object_directory_info = yield* file_system.stat(object_directory);
		const object_format = yield* RunDiscovery(["rev-parse", "--show-object-format"]);

		if (
			object_directory_info.type !== "Directory" ||
			(object_format !== "sha1" && object_format !== "sha256")
		) {
			return yield* Effect.fail(mutation_error("configuration"));
		}

		const identity = yield* HashJson(crypto, {
			git_executable: executable.canonical,
			git_executable_device: executable.info.dev,
			...(Option.isSome(executable.info.ino)
				? { git_executable_inode: executable.info.ino.value }
				: {}),
			git_directory,
			git_directory_device: git_directory_info.dev,
			...(Option.isSome(git_directory_info.ino)
				? { git_directory_inode: git_directory_info.ino.value }
				: {}),
			index_path,
			object_directory,
			object_directory_device: object_directory_info.dev,
			...(Option.isSome(object_directory_info.ino)
				? { object_directory_inode: object_directory_info.ino.value }
				: {}),
			object_format,
			root,
			root_device: root_info.dev,
			...(Option.isSome(root_info.ino) ? { root_inode: root_info.ino.value } : {}),
		});

		return {
			environment: {
				...environment,
				GIT_DIR: git_directory,
				GIT_INDEX_FILE: index_path,
				GIT_WORK_TREE: root,
			},
			git_executable: executable.canonical,
			git_executable_device: executable.info.dev,
			...(Option.isSome(executable.info.ino)
				? { git_executable_inode: executable.info.ino.value }
				: {}),
			git_directory,
			git_directory_device: git_directory_info.dev,
			...(Option.isSome(git_directory_info.ino)
				? { git_directory_inode: git_directory_info.ino.value }
				: {}),
			identity,
			index_path,
			object_directory,
			object_directory_device: object_directory_info.dev,
			...(Option.isSome(object_directory_info.ino)
				? { object_directory_inode: object_directory_info.ino.value }
				: {}),
			object_format,
			root,
			root_device: root_info.dev,
			...(Option.isSome(root_info.ino) ? { root_inode: root_info.ino.value } : {}),
		} satisfies RepositoryPin;
	}).pipe(
		Effect.mapError((cause) =>
			cause instanceof GitMutationError ? cause : mutation_error("configuration", cause),
		),
	);
}

function make_adapter(
	runner: ProcessRunnerShape,
	crypto: Crypto.Crypto,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
	pin: RepositoryPin,
	options: NodeGitMutationOptions,
) {
	const max_stdout_bytes = options.max_stdout_bytes ?? 2 * 1024 * 1024;
	const max_stderr_bytes = options.max_stderr_bytes ?? 256 * 1024;
	const limits = { max_stderr_bytes, max_stdout_bytes };
	const safe_config_arguments = [
		"-c",
		`core.hooksPath=${null_device_path}`,
		"-c",
		"commit.gpgSign=false",
		"-c",
		"core.fsmonitor=false",
		"-c",
		"credential.helper=",
		"-c",
		"gc.auto=0",
		"-c",
		"maintenance.auto=false",
		"-c",
		"merge.gpgSign=false",
		"-c",
		"push.followTags=false",
		"-c",
		"push.gpgSign=false",
		"-c",
		"push.recurseSubmodules=no",
		"-c",
		"rebase.updateRefs=false",
		"-c",
		"rerere.autoupdate=false",
		"-c",
		"rerere.enabled=false",
		"-c",
		"submodule.recurse=false",
	] as const;
	const remote_config_arguments = [
		"-c",
		"http.sslVerify=true",
		"-c",
		"protocol.allow=never",
		"-c",
		"protocol.ext.allow=never",
		"-c",
		"protocol.file.allow=always",
		"-c",
		"protocol.git.allow=never",
		"-c",
		"protocol.http.allow=never",
		"-c",
		"protocol.https.allow=always",
		"-c",
		"protocol.ssh.allow=always",
	] as const;
	const Hash = (value: unknown) => HashJson(crypto, value);
	const HashRaw = (chunks: ReadonlyArray<Uint8Array>) => HashChunks(crypto, chunks);
	const Combine = (first: ProcessRunnerResult, second: ProcessRunnerResult) =>
		combine_results(first, second, limits);
	const transport_base_environment = Object.fromEntries(
		Object.entries(pin.environment).filter(
			([key]) => !["GIT_DIR", "GIT_INDEX_FILE", "GIT_WORK_TREE"].includes(key),
		),
	);
	const Run = (
		args: ReadonlyArray<string>,
		run_options: {
			readonly literal_paths?: boolean;
			readonly remote?: boolean;
			readonly stdin?: Uint8Array;
		} = {},
	) => {
		const command_args = [
			...safe_config_arguments,
			...(run_options.remote ? remote_config_arguments : []),
			...args,
		];

		if (!run_options.remote) {
			return runner.Run({
				args: command_args,
				command: pin.git_executable,
				cwd: pin.root,
				environment: {
					...pin.environment,
					...(run_options.literal_paths ? { GIT_LITERAL_PATHSPECS: "1" } : {}),
				},
				environment_mode: "replace",
				...limits,
				...(run_options.stdin === undefined ? {} : { stdin: run_options.stdin }),
			});
		}

		return Effect.scoped(
			Effect.gen(function* () {
				const transport_directory = yield* file_system.makeTempDirectoryScoped({
					prefix: "artisan-git-transport-",
				});
				const initialized = yield* runner.Run({
					args: [
						"init",
						"--bare",
						"--quiet",
						`--object-format=${pin.object_format}`,
						".",
					],
					command: pin.git_executable,
					cwd: transport_directory,
					environment: transport_base_environment,
					environment_mode: "replace",
					...limits,
				});

				if (
					initialized.exit_code !== 0 ||
					initialized.stdout_truncated ||
					initialized.stderr_truncated
				) {
					return yield* Effect.fail(mutation_error("configuration"));
				}

				return yield* runner.Run({
					args: command_args,
					command: pin.git_executable,
					cwd: transport_directory,
					environment: {
						...transport_base_environment,
						GIT_ALLOW_PROTOCOL: "file:https:ssh",
						GIT_DIR: transport_directory,
						GIT_OBJECT_DIRECTORY: pin.object_directory,
						GIT_PROTOCOL_FROM_USER: "0",
					},
					environment_mode: "replace",
					...limits,
					...(run_options.stdin === undefined ? {} : { stdin: run_options.stdin }),
				});
			}),
		);
	};
	const RawMutationRun = (
		args: ReadonlyArray<string>,
		run_options: {
			readonly literal_paths?: boolean;
			readonly remote?: boolean;
			readonly stdin?: Uint8Array;
		} = {},
	) => Run(args, run_options);
	const ConfigRun = (args: ReadonlyArray<string>) =>
		runner.Run({
			args: ["config", "--no-includes", ...args],
			command: pin.git_executable,
			cwd: pin.root,
			environment: pin.environment,
			environment_mode: "replace",
			...limits,
		});
	const ConfigWriteRun = (args: ReadonlyArray<string>) =>
		runner.Run({
			args: ["config", "--local", "--no-includes", ...args],
			command: pin.git_executable,
			cwd: pin.root,
			environment: pin.environment,
			environment_mode: "replace",
			...limits,
		});
	const Query = (
		args: ReadonlyArray<string>,
		operation: GitMutationError["operation"] = "prepare",
	) =>
		Run(args).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) =>
				result.exit_code === 0 && !result.stdout_truncated && !result.stderr_truncated
					? Effect.succeed(result)
					: Effect.fail(mutation_error(operation)),
			),
		);
	const ConfigValues = (key: string, operation: GitMutationError["operation"] = "prepare") =>
		ConfigRun(["--null", "--get-all", key]).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) => {
				if (result.stdout_truncated || result.stderr_truncated) {
					return Effect.fail(mutation_error(operation));
				}

				if (result.exit_code === 1) {
					return Effect.succeed([] as Array<string>);
				}

				const values = split_nul(result.stdout);

				return result.exit_code === 0 && values !== undefined
					? Effect.succeed(values)
					: Effect.fail(mutation_error(operation));
			}),
		);
	const ConfigSet = (key: string, value: string) =>
		ConfigWriteRun(["--replace-all", key, value]).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
		);
	const ObserveConfiguration = ConfigRun(["--null", "--list"]).pipe(
		Effect.mapError((cause) => mutation_error("process", cause)),
		Effect.flatMap((result) => {
			if (result.exit_code !== 0 || result.stdout_truncated || result.stderr_truncated) {
				return Effect.fail(mutation_error("precondition"));
			}

			const raw_entries = split_nul(result.stdout);

			if (raw_entries === undefined) {
				return Effect.fail(mutation_error("precondition"));
			}

			const entries = raw_entries.map((entry) => {
				const separator = entry.indexOf("\n");

				return separator > 0
					? { key: entry.slice(0, separator), value: entry.slice(separator + 1) }
					: undefined;
			});

			if (
				entries.some(
					(entry) =>
						entry === undefined || is_unsafe_local_config(entry.key, entry.value),
				)
			) {
				return Effect.fail(mutation_error("precondition"));
			}

			return HashRaw([result.stdout]);
		}),
	);
	const MutationRun = (
		args: ReadonlyArray<string>,
		run_options: {
			readonly literal_paths?: boolean;
			readonly remote?: boolean;
			readonly stdin?: Uint8Array;
		} = {},
	) => ObserveConfiguration.pipe(Effect.flatMap(() => RawMutationRun(args, run_options)));
	const RequiredText = (
		args: ReadonlyArray<string>,
		operation: GitMutationError["operation"] = "prepare",
	) =>
		Query(args, operation).pipe(
			Effect.flatMap((result) => {
				const value = clean_text(result);

				return value !== undefined && value.length > 0 && !value.includes("\0")
					? Effect.succeed(value)
					: Effect.fail(mutation_error(operation));
			}),
		);
	const DecodeObjectId = (value: unknown, operation: GitMutationError["operation"] = "prepare") =>
		Schema.decodeUnknownEffect(GitObjectId)(value).pipe(
			Effect.mapError((cause) => mutation_error(operation, cause)),
		);
	const RequiredOid = (
		args: ReadonlyArray<string>,
		operation: GitMutationError["operation"] = "prepare",
	) =>
		RequiredText(args, operation).pipe(
			Effect.flatMap((value) => DecodeObjectId(value, operation)),
		);
	const OptionalOid = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		Run(["rev-parse", "--verify", "--quiet", "--end-of-options", ref]).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) => {
				if (result.stdout_truncated || result.stderr_truncated) {
					return Effect.fail(mutation_error(operation));
				}

				if (result.exit_code === 1) {
					return Effect.succeed(Option.none<string>());
				}

				const value = clean_text(result);

				return result.exit_code === 0 && value !== undefined
					? DecodeObjectId(value, operation).pipe(Effect.map(Option.some))
					: Effect.fail(mutation_error(operation));
			}),
		);
	const CurrentBranch = (operation: GitMutationError["operation"] = "prepare") =>
		Run(["symbolic-ref", "--quiet", "--short", "--no-recurse", "HEAD"]).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) => {
				if (result.stdout_truncated || result.stderr_truncated) {
					return Effect.fail(mutation_error(operation));
				}

				if (result.exit_code === 1) {
					return Effect.succeed(Option.none<string>());
				}

				const value = clean_text(result);

				return result.exit_code === 0 && value !== undefined
					? Schema.decodeUnknownEffect(GitLocalBranchName)(value).pipe(
							Effect.map(Option.some),
							Effect.mapError((cause) => mutation_error(operation, cause)),
						)
					: Effect.fail(mutation_error(operation));
			}),
		);
	const SymbolicTarget = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		Run(["symbolic-ref", "--quiet", "--no-recurse", ref]).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) => {
				if (result.stdout_truncated || result.stderr_truncated) {
					return Effect.fail(mutation_error(operation));
				}

				if (result.exit_code === 1) {
					return Effect.succeed(Option.none<string>());
				}

				const value = clean_text(result);

				return result.exit_code === 0 && value !== undefined && value.startsWith("refs/")
					? Effect.succeed(Option.some(value))
					: Effect.fail(mutation_error(operation));
			}),
		);
	const EnsureDirectRef = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		SymbolicTarget(ref, operation).pipe(
			Effect.flatMap((target) =>
				Option.isNone(target) ? Effect.void : Effect.fail(mutation_error(operation)),
			),
		);
	const EnsurePinned = Effect.gen(function* () {
		const canonical_root = yield* file_system.realPath(pin.root);
		const canonical_git_executable = yield* file_system.realPath(pin.git_executable);
		const canonical_git_directory = yield* file_system.realPath(pin.git_directory);
		const canonical_object_directory = yield* file_system.realPath(pin.object_directory);
		const root_info = yield* file_system.stat(canonical_root);
		const git_executable_info = yield* file_system.stat(canonical_git_executable);
		const git_directory_info = yield* file_system.stat(canonical_git_directory);
		const object_directory_info = yield* file_system.stat(canonical_object_directory);

		if (
			!same_path(canonical_root, pin.root) ||
			!same_path(canonical_git_executable, pin.git_executable) ||
			!same_path(canonical_git_directory, pin.git_directory) ||
			!same_path(canonical_object_directory, pin.object_directory) ||
			!same_pinned_file(pin.root_device, pin.root_inode, root_info) ||
			git_executable_info.type !== "File" ||
			git_executable_info.dev !== pin.git_executable_device ||
			(pin.git_executable_inode !== undefined &&
				Option.getOrUndefined(git_executable_info.ino) !== pin.git_executable_inode) ||
			!same_pinned_file(
				pin.git_directory_device,
				pin.git_directory_inode,
				git_directory_info,
			) ||
			!same_pinned_file(
				pin.object_directory_device,
				pin.object_directory_inode,
				object_directory_info,
			)
		) {
			return yield* Effect.fail(mutation_error("precondition"));
		}
	}).pipe(
		Effect.mapError((cause) =>
			cause instanceof GitMutationError ? cause : mutation_error("precondition", cause),
		),
	);
	const RebaseActive = Effect.gen(function* () {
		const paths = [
			path_service.join(pin.git_directory, "rebase-merge"),
			path_service.join(pin.git_directory, "rebase-apply"),
		];
		const observations = yield* Effect.forEach(paths, (path) =>
			file_system.stat(path).pipe(
				Effect.map(Option.some),
				Effect.catch((cause) =>
					is_platform_reason(cause, "NotFound")
						? Effect.succeed(Option.none<FileSystem.File.Info>())
						: Effect.fail(mutation_error("prepare", cause)),
				),
			),
		);
		const present = observations.filter(Option.isSome);

		if (
			present.length > 1 ||
			present.some((observation) => observation.value.type !== "Directory")
		) {
			return yield* Effect.fail(mutation_error("prepare"));
		}

		return present.length === 1;
	});
	const State = Effect.gen(function* () {
		const merge_head = yield* OptionalOid("MERGE_HEAD");
		const observed_rebase_head = yield* OptionalOid("REBASE_HEAD");
		const original_head = yield* OptionalOid("ORIG_HEAD");
		const rebase_active = yield* RebaseActive;
		const rebase_head = rebase_active ? observed_rebase_head : Option.none<string>();

		if (
			(Option.isSome(merge_head) && rebase_active) ||
			(rebase_active && Option.isNone(observed_rebase_head))
		) {
			return yield* Effect.fail(mutation_error("prepare"));
		}

		return {
			...(Option.isSome(merge_head) ? { merge_head: merge_head.value } : {}),
			...(Option.isSome(original_head) ? { original_head: original_head.value } : {}),
			...(Option.isSome(rebase_head) ? { rebase_head: rebase_head.value } : {}),
			state: Option.isSome(merge_head) ? "merge" : rebase_active ? "rebase" : "none",
		} satisfies GitStateObservation;
	});
	const ObserveSource = Effect.gen(function* () {
		yield* EnsurePinned;

		const configuration_identity = yield* ObserveConfiguration;
		const branch = yield* CurrentBranch();
		const head = yield* RequiredOid(["rev-parse", "--verify", "HEAD"]);

		if (Option.isSome(branch)) {
			const branch_ref = `refs/heads/${branch.value}`;

			yield* EnsureDirectRef(branch_ref);

			if ((yield* RequiredOid(["rev-parse", "--verify", branch_ref])) !== head) {
				return yield* Effect.fail(mutation_error("prepare"));
			}
		}

		const index = yield* Query(["ls-files", "--stage", "-z"]);
		const status = yield* Query(["status", "--porcelain=v1", "-z", "-uall"]);
		const tracked = yield* Query([
			"diff",
			"--binary",
			"--no-ext-diff",
			"--no-textconv",
			"HEAD",
			"--",
		]);
		const untracked = yield* Query(["ls-files", "--others", "--exclude-standard", "-z"]);
		const worktrees = yield* Query(["worktree", "list", "--porcelain", "-z"]);
		const candidates = split_nul(untracked.stdout);

		if (candidates === undefined || candidates.length > 4_096) {
			return yield* Effect.fail(mutation_error("prepare"));
		}

		const contents = yield* Effect.forEach(candidates, (candidate) =>
			MutationRun(["hash-object", "--no-filters", "--", candidate], {
				literal_paths: true,
			}).pipe(
				Effect.mapError((cause) => mutation_error("process", cause)),
				Effect.flatMap((result) => {
					const value = clean_text(result);

					return result.exit_code === 0 &&
						!result.stdout_truncated &&
						!result.stderr_truncated &&
						value !== undefined
						? DecodeObjectId(value)
						: Effect.fail(mutation_error("prepare"));
				}),
			),
		);
		const state = yield* State;

		return {
			...(Option.isSome(branch) ? { branch: branch.value } : {}),
			configuration_identity,
			head,
			index_identity: yield* HashRaw([index.stdout]),
			repository_identity: pin.identity,
			state: state.state,
			state_identity: yield* Hash(state),
			status_identity: yield* HashRaw([status.stdout]),
			tracked_identity: yield* HashRaw([tracked.stdout]),
			untracked_identity: yield* Hash({ candidates, contents }),
			worktree_identity: yield* HashRaw([worktrees.stdout]),
		} satisfies GitMutationSourceProof;
	});
	const Source = Effect.gen(function* () {
		const first = yield* ObserveSource;
		const second = yield* ObserveSource;

		return same_source(first, second)
			? second
			: yield* Effect.fail(mutation_error("precondition"));
	});
	const Resolve = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		RequiredOid(["rev-parse", "--verify", "--end-of-options", `${ref}^{commit}`], operation);
	const ResolveOptional = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		OptionalOid(`${ref}^{commit}`, operation);
	const ResolveDirect = (ref: string, operation: GitMutationError["operation"] = "prepare") =>
		EnsureDirectRef(ref, operation).pipe(Effect.andThen(Resolve(ref, operation)));
	const ResolveDirectOptional = (
		ref: string,
		operation: GitMutationError["operation"] = "prepare",
	) => EnsureDirectRef(ref, operation).pipe(Effect.andThen(ResolveOptional(ref, operation)));
	const RemoteHead = (
		remote_endpoint: string,
		branch: string,
		operation: GitMutationError["operation"] = "prepare",
	) =>
		MutationRun(
			["ls-remote", "--refs", "--exit-code", "--", remote_endpoint, `refs/heads/${branch}`],
			{
				remote: true,
			},
		).pipe(
			Effect.mapError((cause) => mutation_error("process", cause)),
			Effect.flatMap((result) => {
				if (result.stdout_truncated || result.stderr_truncated) {
					return Effect.fail(mutation_error(operation));
				}

				if (result.exit_code === 2) {
					return Effect.succeed(Option.none<string>());
				}

				const value = clean_text(result);
				const [oid, ref, extra] = value?.split(/\s+/u) ?? [];

				return result.exit_code === 0 &&
					oid !== undefined &&
					is_object_id(oid) &&
					ref === `refs/heads/${branch}` &&
					extra === undefined
					? Effect.succeed(Option.some(oid))
					: Effect.fail(mutation_error(operation));
			}),
		);
	const SingleConfigValue = (key: string, operation: GitMutationError["operation"] = "prepare") =>
		ConfigValues(key, operation).pipe(
			Effect.flatMap((values) =>
				values.length === 1 && values[0]!.length > 0 && !has_control_separator(values[0]!)
					? Effect.succeed(values[0]!)
					: Effect.fail(mutation_error(operation)),
			),
		);
	const RemoteEndpoints = (
		remote: string,
		operation: GitMutationError["operation"] = "prepare",
	) =>
		Effect.gen(function* () {
			const urls = yield* ConfigValues(`remote.${remote}.url`, operation);
			const push_urls = yield* ConfigValues(`remote.${remote}.pushurl`, operation);
			const endpoint_bytes = [...urls, ...push_urls].map(
				(endpoint) => encoder.encode(endpoint).byteLength,
			);

			if (
				urls.length !== 1 ||
				push_urls.length > 1 ||
				endpoint_bytes.some((bytes) => bytes > 4096) ||
				urls[0]!.length === 0 ||
				has_control_separator(urls[0]!) ||
				(push_urls[0] !== undefined &&
					(push_urls[0].length === 0 || has_control_separator(push_urls[0])))
			) {
				return yield* Effect.fail(mutation_error(operation));
			}

			const fetch = yield* Schema.decodeUnknownEffect(GitRemoteEndpoint)(urls[0]).pipe(
				Effect.mapError((cause) => mutation_error(operation, cause)),
			);
			const push = yield* Schema.decodeUnknownEffect(GitRemoteEndpoint)(
				push_urls[0] ?? urls[0],
			).pipe(Effect.mapError((cause) => mutation_error(operation, cause)));

			return {
				fetch,
				push,
			};
		});
	const ReadUpstream = (branch: string, operation: GitMutationError["operation"] = "prepare") =>
		Effect.gen(function* () {
			const remote_value = yield* SingleConfigValue(`branch.${branch}.remote`, operation);
			const merge_ref = yield* SingleConfigValue(`branch.${branch}.merge`, operation);
			const remote = yield* Schema.decodeUnknownEffect(GitRemoteName)(remote_value).pipe(
				Effect.mapError((cause) => mutation_error(operation, cause)),
			);

			if (!merge_ref.startsWith("refs/heads/")) {
				return yield* Effect.fail(mutation_error(operation));
			}

			const target_branch = yield* Schema.decodeUnknownEffect(GitLocalBranchName)(
				merge_ref.slice("refs/heads/".length),
			).pipe(Effect.mapError((cause) => mutation_error(operation, cause)));

			return { remote, target_branch };
		});
	const CleanInventory = (operation: GitMutationError["operation"] = "prepare") =>
		Query(
			[
				"ls-files",
				"--others",
				"--exclude-standard",
				"--directory",
				"--no-empty-directory",
				"-z",
			],
			operation,
		).pipe(
			Effect.flatMap((result) => {
				const candidates = split_nul(result.stdout);

				return candidates === undefined
					? Effect.fail(mutation_error(operation))
					: Effect.succeed({ candidates, output: result.stdout });
			}),
		);
	const TargetBranchAvailable = (target_branch: string) =>
		Query(["worktree", "list", "--porcelain", "-z"]).pipe(
			Effect.flatMap((result) => {
				const fields = split_nul(result.stdout);

				return fields === undefined
					? Effect.fail(mutation_error("prepare"))
					: Effect.succeed(!fields.includes(`branch refs/heads/${target_branch}`));
			}),
		);
	const Transaction = (commands: ReadonlyArray<RefTransactionCommand>) =>
		Effect.try({
			try: () => transaction_bytes(commands),
			catch: (cause) => mutation_error("invalid_plan", cause),
		}).pipe(
			Effect.flatMap((stdin) =>
				MutationRun(["update-ref", "--no-deref", "--stdin", "-z"], { stdin }),
			),
		);
	const DecodePlan = (input: unknown, operation: GitMutationError["operation"]) =>
		Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" })(input).pipe(
			Effect.mapError((cause) => mutation_error(operation, cause)),
			Effect.flatMap((plan) => {
				const { binding: _binding, ...unbound } = plan;

				return Hash(unbound).pipe(
					Effect.flatMap((binding) =>
						binding === plan.binding
							? Effect.succeed(plan)
							: Effect.fail(mutation_error("invalid_plan")),
					),
				);
			}),
		);
	const MakePlan = (value: Record<string, unknown>) =>
		Hash(value).pipe(Effect.map((binding) => ({ ...value, binding })));
	const ConflictIdentity = (
		source: GitMutationSourceProof,
		branch: string,
		original_head: string,
		plan_binding: string,
		target_head: string,
		type: "merge" | "rebase",
	) =>
		Hash({
			branch,
			original_head,
			plan_binding,
			source: {
				configuration_identity: source.configuration_identity,
				head: source.head,
				repository_identity: source.repository_identity,
				state: source.state,
				state_identity: source.state_identity,
				worktree_identity: source.worktree_identity,
			},
			target_head,
			type,
		});
	const Prepare = (input: unknown) =>
		Schema.decodeUnknownEffect(GitMutationPreparation, { onExcessProperty: "error" })(
			input,
		).pipe(
			Effect.mapError((cause) => mutation_error("invalid_plan", cause)),
			Effect.flatMap((prepared) =>
				Effect.gen(function* () {
					const contextual = "operation" in prepared;
					const operation = contextual ? prepared.operation : prepared;
					const source = yield* Source.pipe(
						Effect.mapError((cause) => mutation_error("prepare", cause)),
					);
					const continuation =
						(operation.type === "merge" || operation.type === "rebase") &&
						operation.action !== "start";

					if (continuation) {
						if (
							!contextual ||
							source.branch !== undefined ||
							source.state !== operation.type
						) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						const anchor = prepared.action_anchor;
						const identity = yield* ConflictIdentity(
							source,
							anchor.branch,
							anchor.original_head,
							anchor.plan_binding,
							anchor.target_head,
							operation.type,
						);

						if (
							anchor.type !== operation.type ||
							anchor.state !== source.state ||
							anchor.identity !== identity
						) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({ ...operation, anchor, source });
					}

					if (source.branch === undefined || source.state !== "none") {
						return yield* Effect.fail(mutation_error("prepare"));
					}

					if (operation.type === "branch_create") {
						const existing = yield* ResolveDirectOptional(
							`refs/heads/${operation.branch}`,
						);

						if (Option.isSome(existing) || operation.branch === source.branch) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({ ...operation, source, source_head: source.head });
					}

					if (operation.type === "checkout") {
						if (
							operation.target_branch === source.branch ||
							!(yield* TargetBranchAvailable(operation.target_branch))
						) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({
							...operation,
							source,
							target_head: yield* ResolveDirect(
								`refs/heads/${operation.target_branch}`,
							),
						});
					}

					if (operation.type === "reset") {
						return yield* MakePlan({
							...operation,
							source,
							target: yield* Resolve(operation.target),
						});
					}

					if (operation.type === "clean") {
						const inventory = yield* CleanInventory();

						if (inventory.candidates.length === 0) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({
							candidates: inventory.candidates,
							inventory_identity: yield* HashRaw([inventory.output]),
							source,
							type: "clean",
						});
					}

					if (operation.type === "commit") {
						const staged = yield* Run(["diff", "--cached", "--quiet", "HEAD", "--"]);

						if (
							staged.stdout_truncated ||
							staged.stderr_truncated ||
							staged.exit_code !== 1
						) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({ ...operation, source });
					}

					if (
						(operation.type === "merge" || operation.type === "rebase") &&
						operation.action === "start"
					) {
						if (operation.target_branch === source.branch) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						return yield* MakePlan({
							...operation,
							source,
							target_head: yield* ResolveDirect(
								`refs/heads/${operation.target_branch}`,
							),
						});
					}

					if (operation.type === "pull_ff_only") {
						const upstream = yield* ReadUpstream(source.branch);
						const endpoints = yield* RemoteEndpoints(upstream.remote);
						const live = yield* RemoteHead(endpoints.fetch, upstream.target_branch);

						if (Option.isNone(live)) {
							return yield* Effect.fail(mutation_error("prepare"));
						}

						const tracking_ref = `refs/remotes/${upstream.remote}/${upstream.target_branch}`;
						const tracking_head = yield* ResolveDirectOptional(tracking_ref);

						return yield* MakePlan({
							remote: upstream.remote,
							remote_endpoint: endpoints.fetch,
							source,
							target_branch: upstream.target_branch,
							...(Option.isSome(tracking_head)
								? { tracking_head: tracking_head.value }
								: {}),
							upstream_head: live.value,
							type: "pull_ff_only",
						});
					}

					if (operation.type !== "push") {
						return yield* Effect.fail(mutation_error("prepare"));
					}

					const endpoints = yield* RemoteEndpoints(operation.remote);

					if (endpoints.push !== endpoints.fetch) {
						return yield* Effect.fail(mutation_error("prepare"));
					}

					const remote_head = yield* RemoteHead(endpoints.push, operation.target_branch);
					const tracking_head = yield* ResolveDirectOptional(
						`refs/remotes/${operation.remote}/${operation.target_branch}`,
					);

					return yield* MakePlan({
						...operation,
						...(Option.isSome(remote_head)
							? { expected_remote_head: remote_head.value }
							: {}),
						remote_endpoint: endpoints.push,
						source,
						source_branch: source.branch,
						source_head: source.head,
						...(Option.isSome(tracking_head)
							? { tracking_head: tracking_head.value }
							: {}),
					});
				}),
			),
			Effect.flatMap(
				Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" }),
			),
			Effect.mapError((cause) =>
				cause instanceof GitMutationError ? cause : mutation_error("invalid_plan", cause),
			),
		);
	const MutationAttempt = (
		plan: GitMutationPlanType,
		result: ProcessRunnerResult,
		attempt_options: {
			readonly operation_head?: string | undefined;
			readonly phase?: "mutation" | "precondition" | "settlement";
			readonly proof?: GitMutationSourceProof | undefined;
			readonly rejection_reason?: WorkspaceGitMutationRejectionReason | undefined;
		} = {},
	) =>
		Effect.gen(function* () {
			const output_complete = !result.stdout_truncated && !result.stderr_truncated;
			const metadata = encoder.encode(
				canonical({
					stderr_bytes: result.stderr_bytes,
					stderr_truncated: result.stderr_truncated,
					stdout_bytes: result.stdout_bytes,
					stdout_truncated: result.stdout_truncated,
				}),
			);
			const receipt = {
				exit_code: result.exit_code,
				...(attempt_options.operation_head === undefined
					? {}
					: { operation_head: attempt_options.operation_head }),
				output_complete,
				output_identity: yield* HashRaw([metadata, result.stdout, result.stderr]),
				phase: attempt_options.phase ?? ("mutation" as const),
				plan_binding: plan.binding,
				...(attempt_options.rejection_reason === undefined
					? {}
					: { rejection_reason: attempt_options.rejection_reason }),
				...(attempt_options.proof === undefined ? {} : { result: attempt_options.proof }),
				type: "attempt" as const,
			};

			return { ...receipt, binding: yield* Hash(receipt) };
		});
	const Validate = (plan: GitMutationPlanType) =>
		Effect.gen(function* () {
			const current = yield* Source.pipe(
				Effect.mapError((cause) => mutation_error("precondition", cause)),
			);

			if (!same_source(plan.source, current)) {
				return yield* Effect.fail(mutation_error("precondition"));
			}

			if (plan.type === "branch_create") {
				const existing = yield* ResolveDirectOptional(
					`refs/heads/${plan.branch}`,
					"precondition",
				);

				if (Option.isSome(existing)) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "clean") {
				const inventory = yield* CleanInventory("precondition");

				if (
					(yield* HashRaw([inventory.output])) !== plan.inventory_identity ||
					canonical(inventory.candidates) !== canonical(plan.candidates)
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "checkout") {
				if (
					(yield* ResolveDirect(`refs/heads/${plan.target_branch}`, "precondition")) !==
						plan.target_head ||
					!(yield* TargetBranchAvailable(plan.target_branch))
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "reset") {
				if ((yield* Resolve(plan.target, "precondition")) !== plan.target) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "merge" && plan.action === "start") {
				if (
					(yield* ResolveDirect(`refs/heads/${plan.target_branch}`, "precondition")) !==
					plan.target_head
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "rebase" && plan.action === "start") {
				if (
					(yield* ResolveDirect(`refs/heads/${plan.target_branch}`, "precondition")) !==
					plan.target_head
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "pull_ff_only") {
				if (current.branch === undefined) {
					return yield* Effect.fail(mutation_error("precondition"));
				}

				const upstream = yield* ReadUpstream(current.branch, "precondition");
				const endpoints = yield* RemoteEndpoints(plan.remote, "precondition");
				const live = yield* RemoteHead(
					plan.remote_endpoint,
					plan.target_branch,
					"precondition",
				);
				const tracking = yield* ResolveDirectOptional(
					`refs/remotes/${plan.remote}/${plan.target_branch}`,
					"precondition",
				);

				if (
					upstream.remote !== plan.remote ||
					upstream.target_branch !== plan.target_branch ||
					endpoints.fetch !== plan.remote_endpoint ||
					Option.getOrUndefined(live) !== plan.upstream_head ||
					Option.getOrUndefined(tracking) !== plan.tracking_head
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			if (plan.type === "push") {
				const endpoints = yield* RemoteEndpoints(plan.remote, "precondition");
				const live = yield* RemoteHead(
					plan.remote_endpoint,
					plan.target_branch,
					"precondition",
				);
				const tracking = yield* ResolveDirectOptional(
					`refs/remotes/${plan.remote}/${plan.target_branch}`,
					"precondition",
				);

				if (
					endpoints.fetch !== plan.remote_endpoint ||
					endpoints.push !== plan.remote_endpoint ||
					Option.getOrUndefined(tracking) !== plan.tracking_head ||
					Option.getOrUndefined(live) !== plan.expected_remote_head
				) {
					return yield* Effect.fail(mutation_error("precondition"));
				}
			}

			return current;
		});
	const Attach = (
		branch: string,
		source: string,
		target: string,
		extra: ReadonlyArray<RefTransactionCommand> = [],
		head: string = target,
	) =>
		Transaction([
			...extra,
			{ new_oid: target, old_oid: source, ref: `refs/heads/${branch}`, type: "update" },
			{
				old: { type: "oid", value: head },
				ref: "HEAD",
				target: `refs/heads/${branch}`,
				type: "symref_update",
			},
		]);
	const DetachedMatches = (source: GitMutationSourceProof, current: GitMutationSourceProof) =>
		current.branch === undefined &&
		current.configuration_identity === source.configuration_identity &&
		current.head === source.head &&
		current.index_identity === source.index_identity &&
		current.repository_identity === source.repository_identity &&
		current.state === "none" &&
		current.state_identity === source.state_identity &&
		current.tracked_identity === source.tracked_identity &&
		current.untracked_identity === source.untracked_identity;
	const RestoreRejected = (
		plan: GitMutationPlanType,
		result: ProcessRunnerResult,
		reason: WorkspaceGitMutationRejectionReason,
	) =>
		Effect.gen(function* () {
			const current = yield* Source.pipe(Effect.option);

			if (Option.isSome(current) && same_source(current.value, plan.source)) {
				return yield* MutationAttempt(plan, result, {
					proof: current.value,
					rejection_reason: reason,
				});
			}

			if (
				plan.source.branch === undefined ||
				Option.isNone(current) ||
				!DetachedMatches(plan.source, current.value)
			) {
				return yield* MutationAttempt(plan, result, {
					proof: Option.getOrUndefined(current),
				});
			}

			const restored = yield* Transaction([
				{ oid: plan.source.head, ref: `refs/heads/${plan.source.branch}`, type: "verify" },
				{
					old: { type: "oid", value: plan.source.head },
					ref: "HEAD",
					target: `refs/heads/${plan.source.branch}`,
					type: "symref_update",
				},
			]);
			const combined = Combine(result, restored);
			const proof =
				restored.exit_code === 0 ? yield* Source.pipe(Effect.option) : Option.none();

			return yield* MutationAttempt(plan, combined, {
				phase: "settlement",
				proof: Option.getOrUndefined(proof),
				...(Option.isSome(proof) && same_source(proof.value, plan.source)
					? { rejection_reason: reason }
					: {}),
			});
		});
	const Execute = (input: unknown) =>
		DecodePlan(input, "invalid_plan").pipe(
			Effect.flatMap((plan) =>
				Effect.gen(function* () {
					const source = yield* Validate(plan);
					const branch =
						source.branch ??
						((plan.type === "merge" || plan.type === "rebase") &&
						plan.action !== "start"
							? plan.anchor.branch
							: undefined);

					if (plan.type === "branch_create") {
						if (source.branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const detached = yield* MutationRun([
							"switch",
							"--detach",
							"--no-guess",
							plan.source_head,
						]);

						if (detached.exit_code !== 0) {
							return yield* RestoreRejected(plan, detached, "git_rejected");
						}

						const result = yield* Transaction([
							{
								oid: plan.source_head,
								ref: `refs/heads/${source.branch}`,
								type: "verify",
							},
							{
								oid: plan.source_head,
								ref: `refs/heads/${plan.branch}`,
								type: "create",
							},
							{
								old: { type: "oid", value: plan.source_head },
								ref: "HEAD",
								target: `refs/heads/${plan.branch}`,
								type: "symref_update",
							},
						]);
						const accumulated = Combine(detached, result);

						if (result.exit_code !== 0) {
							const existing = yield* ResolveDirectOptional(
								`refs/heads/${plan.branch}`,
								"reconcile",
							).pipe(Effect.option);
							const reason =
								Option.isSome(existing) && Option.isSome(existing.value)
									? "branch_exists"
									: "git_rejected";

							return yield* RestoreRejected(plan, accumulated, reason);
						}

						const proof = yield* Source.pipe(Effect.option);

						return yield* MutationAttempt(plan, accumulated, {
							operation_head: plan.source_head,
							phase: "settlement",
							proof: Option.getOrUndefined(proof),
						});
					}

					if (plan.type === "checkout") {
						if (source.branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const detached = yield* MutationRun([
							"switch",
							"--detach",
							"--no-guess",
							plan.target_head,
						]);

						if (detached.exit_code !== 0) {
							return yield* RestoreRejected(plan, detached, "git_rejected");
						}

						const result = yield* Transaction([
							{
								oid: source.head,
								ref: `refs/heads/${source.branch}`,
								type: "verify",
							},
							{
								oid: plan.target_head,
								ref: `refs/heads/${plan.target_branch}`,
								type: "verify",
							},
							{
								old: { type: "oid", value: plan.target_head },
								ref: "HEAD",
								target: `refs/heads/${plan.target_branch}`,
								type: "symref_update",
							},
						]);
						const proof =
							result.exit_code === 0
								? yield* Source.pipe(Effect.option)
								: Option.none();

						return yield* MutationAttempt(plan, Combine(detached, result), {
							operation_head: result.exit_code === 0 ? plan.target_head : undefined,
							phase: "settlement",
							proof: Option.getOrUndefined(proof),
						});
					}

					if (plan.type === "reset") {
						if (branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const detached = yield* MutationRun([
							"switch",
							"--detach",
							"--no-guess",
							source.head,
						]);

						if (detached.exit_code !== 0) {
							return yield* RestoreRejected(plan, detached, "git_rejected");
						}

						const reset = yield* MutationRun([
							"reset",
							`--${plan.mode}`,
							plan.target,
							"--",
						]);

						if (reset.exit_code !== 0) {
							return yield* RestoreRejected(
								plan,
								Combine(detached, reset),
								"git_rejected",
							);
						}

						const result = yield* Attach(branch, source.head, plan.target);
						const proof =
							result.exit_code === 0
								? yield* Source.pipe(Effect.option)
								: Option.none();

						return yield* MutationAttempt(
							plan,
							Combine(Combine(detached, reset), result),
							{
								operation_head: result.exit_code === 0 ? plan.target : undefined,
								phase: "settlement",
								proof: Option.getOrUndefined(proof),
							},
						);
					}

					if (plan.type === "clean") {
						const result = yield* MutationRun(
							["clean", "-fd", "--", ...plan.candidates],
							{
								literal_paths: true,
							},
						);
						const proof = yield* Source.pipe(Effect.option);

						return yield* MutationAttempt(plan, result, {
							proof: Option.getOrUndefined(proof),
						});
					}

					if (plan.type === "commit") {
						if (branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const detached = yield* MutationRun([
							"switch",
							"--detach",
							"--no-guess",
							source.head,
						]);

						if (detached.exit_code !== 0) {
							return yield* RestoreRejected(plan, detached, "git_rejected");
						}

						const tree = yield* RequiredOid(["write-tree"], "precondition");
						const after_tree = yield* Source;

						if (after_tree.index_identity !== source.index_identity) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const created = yield* MutationRun(
							["commit-tree", tree, "-p", source.head],
							{
								stdin: encoder.encode(`${plan.message}\n`),
							},
						);
						const commit_text = clean_text(created);

						if (
							created.exit_code !== 0 ||
							created.stdout_truncated ||
							created.stderr_truncated ||
							commit_text === undefined ||
							!is_object_id(commit_text)
						) {
							return yield* RestoreRejected(
								plan,
								Combine(detached, created),
								"git_rejected",
							);
						}

						const commit = yield* DecodeObjectId(commit_text, "process");
						const result = yield* Attach(branch, source.head, commit, [], source.head);
						const proof =
							result.exit_code === 0
								? yield* Source.pipe(Effect.option)
								: Option.none();

						return yield* MutationAttempt(
							plan,
							Combine(Combine(detached, created), result),
							{
								operation_head: result.exit_code === 0 ? commit : undefined,
								phase: "settlement",
								proof: Option.getOrUndefined(proof),
							},
						);
					}

					if (plan.type === "merge" || plan.type === "rebase") {
						const original_branch =
							plan.action === "start" ? source.branch : plan.anchor.branch;
						const original_head =
							plan.action === "start" ? source.head : plan.anchor.original_head;

						if (original_branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						let accumulated: ProcessRunnerResult | undefined;

						if (plan.action === "start") {
							const detached = yield* MutationRun([
								"switch",
								"--detach",
								"--no-guess",
								source.head,
							]);

							if (detached.exit_code !== 0) {
								return yield* RestoreRejected(plan, detached, "git_rejected");
							}

							accumulated = detached;
						}

						const args =
							plan.type === "merge"
								? plan.action === "start"
									? [
											"merge",
											"--no-edit",
											"--no-autostash",
											"--no-gpg-sign",
											"--no-verify",
											plan.target_head,
										]
									: ["merge", `--${plan.action}`]
								: plan.action === "start"
									? ["rebase", "--no-autostash", "--no-verify", plan.target_head]
									: ["rebase", `--${plan.action}`];
						const mutation = yield* MutationRun(args);
						const command_result =
							accumulated === undefined ? mutation : Combine(accumulated, mutation);

						const after = yield* Source.pipe(Effect.option);

						if (
							mutation.exit_code !== 0 ||
							Option.isNone(after) ||
							after.value.state !== "none"
						) {
							if (Option.isSome(after) && after.value.state === "none") {
								return yield* RestoreRejected(plan, command_result, "git_rejected");
							}

							return yield* MutationAttempt(plan, command_result, {
								proof: Option.getOrUndefined(after),
							});
						}

						const result = yield* Attach(
							original_branch,
							original_head,
							after.value.head,
						);
						const proof =
							result.exit_code === 0
								? yield* Source.pipe(Effect.option)
								: Option.none();

						return yield* MutationAttempt(plan, Combine(command_result, result), {
							operation_head: result.exit_code === 0 ? after.value.head : undefined,
							phase: "settlement",
							proof: Option.getOrUndefined(proof),
						});
					}

					if (plan.type === "pull_ff_only") {
						if (branch === undefined) {
							return yield* Effect.fail(mutation_error("precondition"));
						}

						const fetched = yield* MutationRun(
							[
								"fetch",
								"--no-recurse-submodules",
								"--no-tags",
								"--no-write-fetch-head",
								"--",
								plan.remote_endpoint,
								plan.upstream_head,
							],
							{ remote: true },
						);

						if (fetched.exit_code !== 0) {
							return yield* MutationAttempt(plan, fetched, {
								proof: source,
								rejection_reason: "remote_rejected",
							});
						}

						const live = yield* RemoteHead(
							plan.remote_endpoint,
							plan.target_branch,
							"precondition",
						);

						if (Option.getOrUndefined(live) !== plan.upstream_head) {
							return yield* MutationAttempt(plan, fetched, {
								proof: source,
								rejection_reason: "remote_changed",
							});
						}

						const ancestor = yield* Run([
							"merge-base",
							"--is-ancestor",
							source.head,
							plan.upstream_head,
						]);

						if (ancestor.exit_code !== 0) {
							return yield* MutationAttempt(plan, Combine(fetched, ancestor), {
								proof: source,
								rejection_reason: "non_fast_forward",
							});
						}

						const detached = yield* MutationRun([
							"switch",
							"--detach",
							"--no-guess",
							plan.upstream_head,
						]);

						if (detached.exit_code !== 0) {
							return yield* RestoreRejected(
								plan,
								Combine(Combine(fetched, ancestor), detached),
								"git_rejected",
							);
						}

						const tracking_ref = `refs/remotes/${plan.remote}/${plan.target_branch}`;
						const tracking_command: RefTransactionCommand =
							plan.tracking_head === undefined
								? { oid: plan.upstream_head, ref: tracking_ref, type: "create" }
								: {
										new_oid: plan.upstream_head,
										old_oid: plan.tracking_head,
										ref: tracking_ref,
										type: "update",
									};
						const result = yield* Attach(branch, source.head, plan.upstream_head, [
							tracking_command,
						]);
						const proof =
							result.exit_code === 0
								? yield* Source.pipe(Effect.option)
								: Option.none();
						const accumulated = Combine(
							Combine(Combine(fetched, ancestor), detached),
							result,
						);

						return yield* MutationAttempt(plan, accumulated, {
							operation_head: result.exit_code === 0 ? plan.upstream_head : undefined,
							phase: "settlement",
							proof: Option.getOrUndefined(proof),
						});
					}

					let accumulated: ProcessRunnerResult | undefined;

					if (plan.expected_remote_head !== undefined) {
						const fetched = yield* MutationRun(
							[
								"fetch",
								"--no-recurse-submodules",
								"--no-tags",
								"--no-write-fetch-head",
								"--",
								plan.remote_endpoint,
								plan.expected_remote_head,
							],
							{ remote: true },
						);

						if (fetched.exit_code !== 0) {
							return yield* MutationAttempt(plan, fetched, {
								proof: source,
								rejection_reason: "remote_rejected",
							});
						}

						const ancestor = yield* Run([
							"merge-base",
							"--is-ancestor",
							plan.expected_remote_head,
							plan.source_head,
						]);
						accumulated = Combine(fetched, ancestor);

						if (ancestor.exit_code !== 0) {
							return yield* MutationAttempt(plan, accumulated, {
								proof: source,
								rejection_reason: "non_fast_forward",
							});
						}
					}

					const expected = plan.expected_remote_head ?? "";
					const pushed = yield* MutationRun(
						[
							"push",
							"--no-follow-tags",
							"--no-verify",
							"--porcelain",
							"--recurse-submodules=no",
							`--force-with-lease=refs/heads/${plan.target_branch}:${expected}`,
							"--",
							plan.remote_endpoint,
							`${plan.source_head}:refs/heads/${plan.target_branch}`,
						],
						{ remote: true },
					);
					accumulated = accumulated === undefined ? pushed : Combine(accumulated, pushed);
					const remote_after = yield* RemoteHead(
						plan.remote_endpoint,
						plan.target_branch,
						"reconcile",
					);

					if (Option.getOrUndefined(remote_after) !== plan.source_head) {
						const reason =
							Option.getOrUndefined(remote_after) === plan.expected_remote_head
								? "remote_rejected"
								: undefined;

						return yield* MutationAttempt(plan, accumulated, {
							proof: source,
							...(reason === undefined ? {} : { rejection_reason: reason }),
						});
					}

					if (!plan.set_upstream) {
						return yield* MutationAttempt(plan, accumulated, {
							operation_head: plan.source_head,
							proof: yield* Source,
						});
					}

					const tracking_ref = `refs/remotes/${plan.remote}/${plan.target_branch}`;
					const tracking = yield* Transaction([
						plan.tracking_head === undefined
							? { oid: plan.source_head, ref: tracking_ref, type: "create" }
							: {
									new_oid: plan.source_head,
									old_oid: plan.tracking_head,
									ref: tracking_ref,
									type: "update",
								},
					]);
					accumulated = Combine(accumulated, tracking);

					if (tracking.exit_code !== 0) {
						return yield* MutationAttempt(plan, accumulated, {
							phase: "settlement",
							proof: yield* Source,
						});
					}

					const remote_set = yield* ConfigSet(
						`branch.${plan.source_branch}.remote`,
						plan.remote,
					);
					accumulated = Combine(accumulated, remote_set);

					if (remote_set.exit_code !== 0) {
						return yield* MutationAttempt(plan, accumulated, {
							phase: "settlement",
							proof: yield* Source,
						});
					}

					const merge_set = yield* ConfigSet(
						`branch.${plan.source_branch}.merge`,
						`refs/heads/${plan.target_branch}`,
					);
					accumulated = Combine(accumulated, merge_set);

					const upstream = yield* ReadUpstream(plan.source_branch, "reconcile").pipe(
						Effect.option,
					);
					const settled =
						merge_set.exit_code === 0 &&
						Option.isSome(upstream) &&
						upstream.value.remote === plan.remote &&
						upstream.value.target_branch === plan.target_branch;

					return yield* MutationAttempt(plan, accumulated, {
						operation_head: settled ? plan.source_head : undefined,
						phase: "settlement",
						proof: yield* Source,
					});
				}),
			),
			Effect.flatMap(
				Schema.decodeUnknownEffect(GitMutationAttempt, { onExcessProperty: "error" }),
			),
			Effect.mapError((cause) =>
				cause instanceof GitMutationError ? cause : mutation_error("process", cause),
			),
		);
	const DecodeAttempt = (input: unknown, plan: GitMutationPlanType) =>
		Schema.decodeUnknownEffect(GitMutationAttempt, { onExcessProperty: "error" })(input).pipe(
			Effect.mapError((cause) => mutation_error("reconcile", cause)),
			Effect.flatMap((attempt) => {
				const { binding: _binding, ...unbound } = attempt;

				return Hash(unbound).pipe(
					Effect.flatMap((binding) =>
						binding === attempt.binding && attempt.plan_binding === plan.binding
							? Effect.succeed(attempt)
							: Effect.fail(mutation_error("reconcile")),
					),
				);
			}),
		);
	const ReadSmallOid = (path: string) =>
		Effect.scoped(
			Effect.gen(function* () {
				const file = yield* file_system.open(path, { flag: "r" });
				const stat = yield* file.stat;

				if (stat.type !== "File" || stat.size > 256n) {
					return yield* Effect.fail(mutation_error("reconcile"));
				}

				const bytes =
					stat.size === 0n
						? new Uint8Array()
						: Option.getOrElse(
								yield* file.readAlloc(stat.size),
								() => new Uint8Array(),
							);
				const value = decode_utf8(bytes)?.trim();

				return value !== undefined && is_object_id(value)
					? value
					: yield* Effect.fail(mutation_error("reconcile"));
			}),
		).pipe(
			Effect.mapError((cause) =>
				cause instanceof GitMutationError ? cause : mutation_error("reconcile", cause),
			),
		);
	const RebaseOnto = Effect.gen(function* () {
		const candidates = [
			path_service.join(pin.git_directory, "rebase-merge", "onto"),
			path_service.join(pin.git_directory, "rebase-apply", "onto"),
		];
		const values = yield* Effect.forEach(candidates, (path) =>
			ReadSmallOid(path).pipe(Effect.option),
		);
		const present = values.filter(Option.isSome).map((value) => value.value);

		return present.length === 1 ? Option.some(present[0]!) : Option.none<string>();
	});
	const MatchConflict = (plan: GitMutationPlanType, current: GitMutationSourceProof) =>
		Effect.gen(function* () {
			if (
				(plan.type !== "merge" && plan.type !== "rebase") ||
				current.branch !== undefined ||
				current.state !== plan.type
			) {
				return Option.none<GitMutationActionAnchor>();
			}

			const state = yield* State;
			const branch = plan.action === "start" ? plan.source.branch : plan.anchor.branch;
			const original_head =
				plan.action === "start" ? plan.source.head : plan.anchor.original_head;
			const target_head =
				plan.action === "start" ? plan.target_head : plan.anchor.target_head;

			if (
				branch === undefined ||
				state.original_head !== original_head ||
				(plan.type === "merge" && state.merge_head !== target_head)
			) {
				return Option.none<GitMutationActionAnchor>();
			}

			if (plan.type === "rebase") {
				const onto = yield* RebaseOnto;

				if (Option.getOrUndefined(onto) !== target_head) {
					return Option.none<GitMutationActionAnchor>();
				}
			}

			const anchor = {
				branch,
				identity: yield* ConflictIdentity(
					current,
					branch,
					original_head,
					plan.binding,
					target_head,
					plan.type,
				),
				original_head,
				plan_binding: plan.binding,
				state: plan.type,
				target_head,
				type: plan.type,
			} satisfies GitMutationActionAnchor;

			return Option.some(anchor);
		});
	const Expected = (
		plan: GitMutationPlanType,
		proof: GitMutationSourceProof,
		operation_head?: string,
	) => {
		if (plan.type === "branch_create") {
			return (
				operation_head === plan.source_head &&
				proof.branch === plan.branch &&
				proof.head === plan.source_head
			);
		}

		if (plan.type === "checkout") {
			return (
				operation_head === plan.target_head &&
				proof.branch === plan.target_branch &&
				proof.head === plan.target_head
			);
		}

		if (plan.type === "reset") {
			return (
				operation_head === plan.target &&
				proof.branch === plan.source.branch &&
				proof.head === plan.target
			);
		}

		if (plan.type === "pull_ff_only") {
			return (
				operation_head === plan.upstream_head &&
				proof.branch === plan.source.branch &&
				proof.head === plan.upstream_head
			);
		}

		if (plan.type === "clean") {
			return (
				proof.branch === plan.source.branch &&
				proof.configuration_identity === plan.source.configuration_identity &&
				proof.head === plan.source.head &&
				proof.index_identity === plan.source.index_identity &&
				proof.repository_identity === plan.source.repository_identity &&
				proof.state === "none" &&
				proof.tracked_identity === plan.source.tracked_identity
			);
		}

		if (plan.type === "commit") {
			return (
				operation_head !== undefined &&
				proof.branch === plan.source.branch &&
				proof.head === operation_head &&
				proof.state === "none"
			);
		}

		if (plan.type === "merge" || plan.type === "rebase") {
			const branch = plan.action === "start" ? plan.source.branch : plan.anchor.branch;
			const original_head =
				plan.action === "start" ? plan.source.head : plan.anchor.original_head;

			return (
				proof.branch === branch &&
				proof.state === "none" &&
				(plan.action === "abort"
					? proof.head === original_head
					: operation_head !== undefined && proof.head === operation_head)
			);
		}

		return proof.state === "none";
	};
	const PushUpstreamMatches = (plan: Extract<GitMutationPlanType, { type: "push" }>) =>
		plan.set_upstream
			? Effect.gen(function* () {
					const upstream = yield* ReadUpstream(plan.source_branch, "reconcile");
					const tracking = yield* ResolveDirectOptional(
						`refs/remotes/${plan.remote}/${plan.target_branch}`,
						"reconcile",
					);

					return (
						upstream.remote === plan.remote &&
						upstream.target_branch === plan.target_branch &&
						Option.getOrUndefined(tracking) === plan.source_head
					);
				}).pipe(Effect.catch(() => Effect.succeed(false)))
			: Effect.succeed(true);
	const Reconcile = (input: unknown, receipt?: unknown) =>
		DecodePlan(input, "invalid_plan").pipe(
			Effect.flatMap((plan) =>
				Effect.gen(function* () {
					const current = yield* Source.pipe(
						Effect.mapError((cause) => mutation_error("reconcile", cause)),
					);
					const attempt =
						receipt === undefined
							? Option.none<GitMutationAttempt>()
							: Option.some(yield* DecodeAttempt(receipt, plan));
					const conflict = yield* MatchConflict(plan, current);

					if (
						Option.isSome(conflict) &&
						(Option.isNone(attempt) ||
							(attempt.value.output_complete &&
								attempt.value.result !== undefined &&
								same_source(attempt.value.result, current)))
					) {
						return {
							action: plan.type === "merge" ? "merge_conflict" : "rebase_conflict",
							anchor: conflict.value,
							type: "action_required",
						} as const;
					}

					if (Option.isNone(attempt)) {
						return same_source(plan.source, current)
							? ({ type: "source" } as const)
							: ({ type: "outcome_unknown" } as const);
					}

					if (
						attempt.value.rejection_reason !== undefined &&
						attempt.value.result !== undefined &&
						same_source(attempt.value.result, current) &&
						same_source(plan.source, current)
					) {
						return {
							reason: attempt.value.rejection_reason,
							type: "rejected",
						} as const;
					}

					const remote_head =
						plan.type === "push"
							? Option.getOrUndefined(
									yield* RemoteHead(
										plan.remote_endpoint,
										plan.target_branch,
										"reconcile",
									),
								)
							: undefined;
					const upstream_matches =
						plan.type === "push" ? yield* PushUpstreamMatches(plan) : true;
					const push_applied =
						plan.type === "push" &&
						attempt.value.operation_head === plan.source_head &&
						remote_head === plan.source_head &&
						upstream_matches;
					const clean_inventory =
						plan.type === "clean"
							? yield* CleanInventory("reconcile").pipe(Effect.option)
							: Option.none<{ candidates: Array<string>; output: Uint8Array }>();
					const clean_applied =
						plan.type !== "clean" ||
						(Option.isSome(clean_inventory) &&
							plan.candidates.every(
								(candidate) =>
									!clean_inventory.value.candidates.includes(candidate),
							));
					const local_applied =
						attempt.value.exit_code === 0 &&
						attempt.value.result !== undefined &&
						same_source(attempt.value.result, current) &&
						clean_applied &&
						Expected(plan, current, attempt.value.operation_head);

					if (!attempt.value.output_complete || (!push_applied && !local_applied)) {
						return { type: "outcome_unknown" } as const;
					}

					return {
						...(current.branch === undefined ? {} : { branch: current.branch }),
						head: current.head,
						...(remote_head === undefined ? {} : { remote_head }),
						type: "applied",
					} as const;
				}),
			),
		) as Effect.Effect<GitMutationReconciliation, GitMutationError>;
	return { Execute, Prepare, Reconcile };
}

/** Builds an injectable Git mutation layer from Effect platform capabilities. */
export function make_git_mutation_layer(options: NodeGitMutationOptions) {
	const max_stdout_bytes = options.max_stdout_bytes ?? 2 * 1024 * 1024;
	const max_stderr_bytes = options.max_stderr_bytes ?? 256 * 1024;

	return Layer.effect(
		GitMutation,
		Effect.gen(function* () {
			if (!is_valid_limit(max_stdout_bytes) || !is_valid_limit(max_stderr_bytes)) {
				return yield* Effect.fail(mutation_error("configuration"));
			}

			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const runner = yield* ProcessRunner;
			const crypto = yield* Crypto.Crypto;
			const GetPin = yield* Effect.cached(
				BuildRepositoryPin(runner, crypto, file_system, path_service, options),
			);
			const GetAdapter = GetPin.pipe(
				Effect.map((pin) =>
					make_adapter(runner, crypto, file_system, path_service, pin, options),
				),
			);

			return {
				Execute: (plan: unknown) =>
					GetAdapter.pipe(Effect.flatMap((adapter) => adapter.Execute(plan))),
				Prepare: (operation: unknown) =>
					GetAdapter.pipe(Effect.flatMap((adapter) => adapter.Prepare(operation))),
				Reconcile: (plan: unknown, attempt?: unknown) =>
					GetAdapter.pipe(Effect.flatMap((adapter) => adapter.Reconcile(plan, attempt))),
			};
		}),
	);
}

/** Builds the production Git mutation layer with bounded Node and Effect platform services. */
export function make_node_git_mutation_layer(options: NodeGitMutationOptions) {
	return make_git_mutation_layer(options).pipe(
		Layer.provide(make_node_process_runner_layer(options.process)),
		Layer.provide(NodeCrypto.layer),
		Layer.provide(NodeFileSystem.layer),
		Layer.provide(NodePath.layer),
	);
}
