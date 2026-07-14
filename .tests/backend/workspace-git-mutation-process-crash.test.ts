import { execFile, spawn } from "node:child_process";
import { chmod, mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, Layer, ManagedRuntime, Option, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	Git,
	GitMutation,
	NodeProcessRunnerLive,
	ProcessRunner,
	WorkspaceGitMutationCoordinator,
	WorkspaceGitMutationRepository,
	WorkspaceGitRegistry,
	WorkspaceGitSessionService,
} from "@artisan/backend";
import { WorkspaceGitMutationCoordinatorLive } from "../../modules/backend/src/git/workspace-git-mutation-coordinator";
import { WorkspaceGitMutationRepositoryLive } from "../../modules/backend/src/git/workspace-git-mutation-repository";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { WorkspaceGitObserverLive } from "../../modules/backend/src/git/workspace-git-observer";
import { WorkspaceGitSessionRepositoryLive } from "../../modules/backend/src/git/workspace-git-session-repository";
import { WorkspaceGitSessionServiceLive } from "../../modules/backend/src/git/workspace-git-session-service";
import { WorkspaceGitMutationClaims } from "../../modules/backend/src/persistence/schema";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	GitMutationError,
	GitMutationPlan,
	GitMutationPreparation,
} from "../../modules/backend/src/git/git-mutation";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { WorkspaceEvidenceRecorder } from "../../modules/backend/src/workspace/workspace-evidence-recorder";

const exec_file = promisify(execFile);
const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const fixture_path = fileURLToPath(
	new URL("./workspace-git-mutation-process-crash-fixture.ts", import.meta.url),
);
const typescript_loader_url = new URL("./orchestration-agent-graph-loader.mjs", import.meta.url)
	.href;
const workspace_id = "workspace_git_mutation_crash";
const thread_id = "thread_git_mutation_crash";
const original_approval_id = "workspace_git_mutation:request_git_mutation_crash";
const cleanup_timeout_ms = 45_000;
const credential_timeout_ms = 5_000;
const process_enumeration_timeout_ms = 2_000;
const socket_directory_prefix = "ae-git-mutation-";
const socket_path_prefix = "ae-git-";
const GitProcess = Schema.Struct({
	command_line: Schema.NonEmptyString,
	state: Schema.NonEmptyString,
});
const GitProcessOutput = Schema.Union([GitProcess, Schema.Array(GitProcess)]);
const active_cleanups = new Set<HarnessCleanup>();
const text_decoder = new TextDecoder();
const text_encoder = new TextEncoder();

type GitProcessValue = typeof GitProcess.Type;
type DispatcherAttempt = Deferred.Deferred<{
	readonly approval_id: string;
	readonly exit: Exit.Exit<unknown, unknown>;
}>;

interface HarnessCleanup {
	directory: string;
	fixture?: ReturnType<typeof spawn>;
	cleanup?: Promise<void>;
	runtime?: ManagedRuntime.ManagedRuntime<any, any>;
	socket_path?: string;
	socket_directory?: string;
}

function make_source(head: string) {
	return {
		branch: "main",
		configuration_identity: "1".repeat(64),
		head,
		index_identity: "2".repeat(64),
		repository_identity: "3".repeat(64),
		state: "none" as const,
		state_identity: "4".repeat(64),
		status_identity: "5".repeat(64),
		tracked_identity: "6".repeat(64),
		untracked_identity: "7".repeat(64),
		worktree_identity: "8".repeat(64),
	};
}

function make_metadata_layer(instance_id: string, now: string) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id,
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
		Now: Effect.succeed(now),
	});
}

function delay(milliseconds: number) {
	return new Promise<void>((resolve) => setTimeout(resolve, milliseconds));
}

async function run_git(cwd: string, args: ReadonlyArray<string>) {
	return exec_file("git", [...args], {
		cwd,
		timeout: 10_000,
		windowsHide: true,
	});
}

async function make_repository() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-git-mutation-process-crash-"));
	const root = join(directory, "repository");

	try {
		await mkdir(root, { recursive: true });
		await run_git(root, ["init", "-b", "main"]);
		await run_git(root, ["config", "user.email", "crash@example.test"]);
		await run_git(root, ["config", "user.name", "Crash Harness"]);
		await writeFile(join(root, "initial.txt"), "initial\n");
		await run_git(root, ["add", "initial.txt"]);
		await run_git(root, ["commit", "-m", "initial"]);
		const source_head = (await run_git(root, ["rev-parse", "HEAD"])).stdout.trim();

		await writeFile(join(root, "mutation.txt"), "staged mutation\n");
		await run_git(root, ["add", "mutation.txt"]);

		return { database_path: join(directory, "artisan.db"), directory, root, source_head };
	} catch (cause) {
		assert_harness_directory(directory);
		await remove_harness_path(
			directory,
			"Crash harness setup cleanup",
			Date.now() + credential_timeout_ms,
		);

		throw cause;
	}
}

function remaining_timeout_ms(deadline: number, maximum_ms: number) {
	return Math.max(1, Math.min(maximum_ms, deadline - Date.now()));
}

function delay_within_deadline(deadline: number, maximum_ms: number) {
	return delay(remaining_timeout_ms(deadline, maximum_ms));
}

function assert_harness_directory(directory: string) {
	if (
		dirname(directory) !== tmpdir() ||
		!basename(directory).startsWith("artisan-git-mutation-process-crash-")
	) {
		throw new Error(`Refusing to remove a non-harness directory: ${directory}`);
	}
}

function assert_harness_socket_path(socket_path: string, socket_directory?: string) {
	if (socket_directory !== undefined) {
		assert_harness_socket_directory(socket_directory);

		if (socket_path !== join(socket_directory, "s")) {
			throw new Error(`Refusing to remove a non-harness socket: ${socket_path}`);
		}

		return;
	}

	if (
		dirname(socket_path) !== tmpdir() ||
		!basename(socket_path).startsWith(socket_path_prefix) ||
		!socket_path.endsWith(".sock")
	) {
		throw new Error(`Refusing to remove a non-harness socket: ${socket_path}`);
	}
}

function assert_harness_socket_directory(socket_directory: string) {
	if (
		dirname(socket_directory) !== tmpdir() ||
		!basename(socket_directory).startsWith(socket_directory_prefix)
	) {
		throw new Error(`Refusing to remove a non-harness socket directory: ${socket_directory}`);
	}
}

function kill_parent_process(child: ReturnType<typeof spawn>) {
	if (child.exitCode !== null || child.signalCode !== null) {
		return;
	}

	if (!child.kill("SIGKILL")) {
		throw new Error("Git mutation crash fixture parent could not be terminated");
	}
}

async function wait_for_exit(
	child: ReturnType<typeof spawn>,
	description: string,
	timeout_ms = 10_000,
) {
	if (child.exitCode !== null || child.signalCode !== null) {
		return;
	}

	await new Promise<void>((resolve, reject) => {
		const timeout = setTimeout(
			() =>
				complete(() =>
					reject(new Error(`${description} did not complete within ${timeout_ms}ms`)),
				),
			timeout_ms,
		);
		const complete = (effect: () => void) => {
			clearTimeout(timeout);
			child.removeListener("error", on_error);
			child.removeListener("exit", on_exit);
			effect();
		};
		const on_error = (cause: Error) => complete(() => reject(cause));
		const on_exit = () => complete(resolve);

		child.once("error", on_error);
		child.once("exit", on_exit);
	});
}

async function terminate_fixture(child: ReturnType<typeof spawn>, timeout_ms: number) {
	if (child.exitCode !== null || child.signalCode !== null) {
		return;
	}

	child.kill("SIGKILL");
	await wait_for_exit(child, "Git mutation crash fixture cleanup", timeout_ms);
}

function decode_git_processes(output: string) {
	if (output.trim().length === 0) {
		return [];
	}

	const decoded = Schema.decodeUnknownSync(GitProcessOutput)(JSON.parse(output));

	return Array.isArray(decoded) ? decoded : [decoded];
}

async function run_harness_process(
	input: Parameters<typeof ProcessRunner.Service.Run>[0],
	description: string,
	timeout_ms: number,
) {
	const result = await Effect.runPromise(
		ProcessRunner.pipe(
			Effect.flatMap((process_runner) => process_runner.Run(input)),
			Effect.provide(NodeProcessRunnerLive),
			Effect.timeoutOrElse({
				duration: timeout_ms,
				orElse: () =>
					Effect.fail(new Error(`${description} timed out after ${timeout_ms}ms`)),
			}),
		),
	);

	if (result.exit_code !== 0) {
		throw new Error(
			`${description} exited with ${result.exit_code}: ${text_decoder.decode(result.stderr)}`,
		);
	}

	return result;
}

async function windows_git_processes(socket_path: string, timeout_ms: number) {
	const script = [
		"$ErrorActionPreference = 'Stop'",
		"$socket_path = $env:ARTISAN_GIT_MUTATION_SOCKET_PATH",
		"$matches = Get-CimInstance Win32_Process -Filter \"Name = 'git.exe'\" | Where-Object { $_.CommandLine -and $_.CommandLine.IndexOf($socket_path, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 }",
		"$matches | ForEach-Object { [PSCustomObject]@{ command_line = [string]$_.CommandLine; state = 'running' } } | ConvertTo-Json -Compress",
	].join("; ");
	const result = await run_harness_process(
		{
			args: ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script],
			command: "powershell.exe",
			cwd: process.cwd(),
			environment: { ARTISAN_GIT_MUTATION_SOCKET_PATH: socket_path },
			max_stderr_bytes: 64 * 1024,
			max_stdout_bytes: 64 * 1024,
		},
		"Git daemon process enumeration",
		timeout_ms,
	);

	return decode_git_processes(text_decoder.decode(result.stdout));
}

async function posix_git_processes(socket_path: string, timeout_ms: number) {
	const result = await run_harness_process(
		{
			args: ["-axo", "pid=,state=,lstart=,command="],
			command: "ps",
			cwd: process.cwd(),
			max_stderr_bytes: 64 * 1024,
			max_stdout_bytes: 4 * 1024 * 1024,
		},
		"Git daemon process enumeration",
		timeout_ms,
	);
	const stdout = text_decoder.decode(result.stdout);
	const processes: Array<GitProcessValue> = [];
	const line_pattern =
		/^\s*(\d+)\s+(\S+)\s+(\S+\s+\S+\s+\d+\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(.+)$/u;

	for (const line of stdout.split(/\r?\n/u)) {
		const match = line_pattern.exec(line);

		if (!match) {
			continue;
		}

		const command_line = match[4]!;
		const state = match[2]!;

		if (
			state.startsWith("Z") ||
			!command_line.includes(socket_path) ||
			!command_line.includes("credential-cache--daemon")
		) {
			continue;
		}

		processes.push(
			Schema.decodeUnknownSync(GitProcess)({
				command_line,
				state,
			}),
		);
	}

	return processes;
}

function CredentialCacheAction(
	socket_path: string,
	action: "exit" | "get",
	timeout_ms: number,
	credential_id?: string,
) {
	const input =
		action === "get" && credential_id !== undefined
			? text_encoder.encode(
					[
						"protocol=https",
						"host=artisan-editor.test",
						`username=${credential_id}`,
						"",
					].join("\n"),
				)
			: undefined;

	return Effect.gen(function* () {
		const process_runner = yield* ProcessRunner;
		const result = yield* process_runner.Run({
			args: ["credential-cache", "--socket", socket_path, action],
			command: "git",
			cwd: process.cwd(),
			max_stderr_bytes: 64 * 1024,
			max_stdout_bytes: 64 * 1024,
			...(input === undefined ? {} : { stdin: input }),
		});

		if (result.exit_code !== 0) {
			return yield* Effect.fail(
				new Error(
					`Credential cache ${action} failed: ${text_decoder.decode(result.stderr)}`,
				),
			);
		}

		return text_decoder.decode(result.stdout);
	}).pipe(
		Effect.provide(NodeProcessRunnerLive),
		Effect.timeoutOrElse({
			duration: timeout_ms,
			orElse: () =>
				Effect.fail(
					new Error(
						`Credential cache ${action} timed out after ${timeout_ms}ms at ${socket_path}`,
					),
				),
		}),
	);
}

async function credential_cache_action(
	socket_path: string,
	action: "exit" | "get",
	credential_id?: string,
	timeout_ms = credential_timeout_ms,
) {
	return Effect.runPromise(CredentialCacheAction(socket_path, action, timeout_ms, credential_id));
}

async function exit_credential_cache(socket_path: string, deadline: number) {
	let last_failure: unknown;
	let absent_observations = 0;

	while (Date.now() < deadline) {
		let processes: ReadonlyArray<GitProcessValue>;

		try {
			processes = await git_processes_for_socket(
				socket_path,
				remaining_timeout_ms(deadline, process_enumeration_timeout_ms),
			);
		} catch (cause) {
			last_failure = cause;
			await delay_within_deadline(deadline, 100);

			continue;
		}

		if (processes.length === 0) {
			absent_observations += 1;

			if (absent_observations === 3) {
				return;
			}

			await delay_within_deadline(deadline, 100);

			continue;
		}

		absent_observations = 0;

		try {
			await credential_cache_action(
				socket_path,
				"exit",
				undefined,
				remaining_timeout_ms(deadline, credential_timeout_ms),
			);
		} catch (cause) {
			last_failure = cause;
		}

		await delay_within_deadline(deadline, 100);
	}

	throw new Error(`Git credential-cache daemon did not exit for ${socket_path}`, {
		cause: last_failure,
	});
}

async function remove_harness_path(path: string, description: string, deadline: number) {
	const verification_script = [
		'import { lstat } from "node:fs/promises";',
		"try {",
		"await lstat(process.argv[1]);",
		'throw new Error("Harness-owned path still exists after removal");',
		"} catch (cause) {",
		'if (cause === null || typeof cause !== "object" || !("code" in cause) || cause.code !== "ENOENT") throw cause;',
		"}",
	].join(" ");
	const input =
		process.platform === "win32"
			? {
					args: [
						"-NoLogo",
						"-NoProfile",
						"-NonInteractive",
						"-Command",
						"Remove-Item -LiteralPath $env:ARTISAN_GIT_MUTATION_REMOVE_PATH -Force -Recurse -ErrorAction SilentlyContinue; exit 0",
					],
					command: "powershell.exe",
					cwd: process.cwd(),
					environment: { ARTISAN_GIT_MUTATION_REMOVE_PATH: path },
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
				}
			: {
					args: ["-rf", "--", path],
					command: "rm",
					cwd: process.cwd(),
					max_stderr_bytes: 64 * 1024,
					max_stdout_bytes: 64 * 1024,
				};

	await run_harness_process(
		input,
		description,
		remaining_timeout_ms(deadline, credential_timeout_ms),
	);
	await run_harness_process(
		{
			args: ["--input-type=module", "-e", verification_script, path],
			command: process.execPath,
			cwd: process.cwd(),
			max_stderr_bytes: 64 * 1024,
			max_stdout_bytes: 64 * 1024,
		},
		`${description} verification`,
		remaining_timeout_ms(deadline, credential_timeout_ms),
	);
}

async function remove_socket(
	socket_path: string,
	socket_directory: string | undefined,
	deadline: number,
) {
	assert_harness_socket_path(socket_path, socket_directory);

	if (process.platform === "win32") {
		await remove_harness_path(socket_path, "Git socket cleanup", deadline);

		return;
	}

	await remove_harness_path(socket_path, "Git socket cleanup", deadline);
}

function git_processes_for_socket(socket_path: string, timeout_ms: number) {
	return process.platform === "win32"
		? windows_git_processes(socket_path, timeout_ms)
		: posix_git_processes(socket_path, timeout_ms);
}

async function wait_for_git_processes(socket_path: string) {
	const deadline = Date.now() + credential_timeout_ms;

	while (Date.now() < deadline) {
		const processes = await git_processes_for_socket(
			socket_path,
			remaining_timeout_ms(deadline, process_enumeration_timeout_ms),
		);

		if (processes.length > 0) {
			return processes;
		}

		await delay_within_deadline(deadline, 20);
	}

	throw new Error(
		`Git daemon was not running within ${credential_timeout_ms}ms for socket ${socket_path}`,
	);
}

async function wait_for_readiness(child: ReturnType<typeof spawn>) {
	if (child.stderr === null || child.stdout === null) {
		throw new Error("Git mutation crash fixture did not expose output streams");
	}

	const stderr_stream = child.stderr;
	const stdout_stream = child.stdout;

	await new Promise<void>((resolve, reject) => {
		let stderr = "";
		let stdout = "";
		const timeout = setTimeout(
			() =>
				complete(() =>
					reject(
						new Error(
							`Git mutation crash fixture readiness did not complete within 15000ms: ${stderr}`,
						),
					),
				),
			15_000,
		);
		const cleanup = () => {
			clearTimeout(timeout);
			child.removeListener("error", on_error);
			child.removeListener("exit", on_exit);
			stderr_stream.removeListener("data", on_stderr);
			stdout_stream.removeListener("data", on_stdout);
		};
		const complete = (effect: () => void) => {
			cleanup();
			effect();
		};
		const on_error = (cause: Error) => complete(() => reject(cause));
		const on_exit = (code: number | null, signal: NodeJS.Signals | null) =>
			complete(() =>
				reject(
					new Error(
						`Git mutation crash fixture exited before readiness (${code ?? signal}): ${stderr}`,
					),
				),
			);
		const on_stderr = (chunk: Buffer) => {
			stderr += String(chunk);
		};
		const on_stdout = (chunk: Buffer) => {
			stdout += String(chunk);

			if (stdout.includes("GIT_MUTATION_CRASH_READY")) {
				complete(resolve);
			}
		};

		child.once("error", on_error);
		child.once("exit", on_exit);
		stderr_stream.on("data", on_stderr);
		stdout_stream.on("data", on_stdout);
	});
}

function make_recovery_registry(
	root: string,
	source_head: string,
	illegal_calls: { execute: number; reconcile: number },
) {
	const source = make_source(source_head);
	const read: typeof Git.Service = {
		DiffPatch: () => Effect.succeed({ bytes: 0, patch: "", truncated: false }),
		DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
		Discover: Effect.succeed({ branch: "main", head: Option.some(source.head), root }),
		ProbeRepository: Effect.succeed(
			Option.some({ branch: "main", head: Option.some(source.head), root }),
		),
		ResolveLocalBranch: () => Effect.succeed(Option.none()),
		Status: Effect.succeed([]),
		Worktrees: Effect.succeed([
			{
				adapter_path: root,
				bare: false,
				branch: Option.some("refs/heads/main"),
				detached: false,
				head: Option.some(source.head),
				locked: false,
				prunable: false,
			},
		]),
	};
	const Prepare = (input: unknown) =>
		Effect.try({
			try: () => {
				const preparation = Schema.decodeUnknownSync(GitMutationPreparation)(input);
				const operation = "operation" in preparation ? preparation.operation : preparation;

				if (operation.type !== "commit") {
					throw new Error("Only commit preparation is used by this fixture");
				}

				return Schema.decodeUnknownSync(GitMutationPlan)({
					binding: "d".repeat(64),
					message: operation.message,
					source,
					type: "commit",
				});
			},
			catch: (cause) => new GitMutationError({ cause, operation: "invalid_plan" }),
		});
	const Execute = () =>
		Effect.sync(() => {
			illegal_calls.execute += 1;
			throw new Error("Recovery attempted to execute a fenced Git mutation");
		});
	const Reconcile = () =>
		Effect.sync(() => {
			illegal_calls.reconcile += 1;
			throw new Error("Recovery attempted to reconcile an interrupted Git mutation");
		});
	const mutation: typeof GitMutation.Service = { Execute, Prepare, Reconcile };

	return Layer.succeed(WorkspaceGitRegistry, {
		Get: (requested_workspace_id) =>
			requested_workspace_id === workspace_id
				? Effect.succeed({
						canonical_root: root,
						mutation,
						read,
						workspace_id,
					})
				: Effect.die(`Unexpected workspace ${requested_workspace_id}`),
		ListWorkspaceIds: Effect.succeed([workspace_id]),
	});
}

async function read_claim_snapshot(database_path: string) {
	const runtime = ManagedRuntime.make(make_database_layer({ database_path, migrations_path }));

	try {
		const claims = await runtime.runPromise(
			Effect.flatMap(Database, (database) =>
				database.client.select().from(WorkspaceGitMutationClaims),
			),
		);
		const claim = claims.find(({ approval_id }) => approval_id === original_approval_id);

		if (!claim) {
			throw new Error("Executing Git mutation claim was not persisted before readiness");
		}

		return claim;
	} finally {
		await Effect.runPromise(runtime.disposeEffect.pipe(Effect.timeout("10 seconds")));
	}
}

async function read_claims(runtime: ManagedRuntime.ManagedRuntime<any, any>) {
	return runtime.runPromise(
		Effect.flatMap(Database, (database) =>
			database.client.select().from(WorkspaceGitMutationClaims),
		),
	);
}

function make_evidence_layer() {
	return Layer.succeed(WorkspaceEvidenceRecorder, {
		RecordFilesystemMutation: () => Effect.die("unused"),
		RecordGitWorkspaceObserved: () =>
			Effect.succeed({ event: {} as never, status: "accepted" as const }),
		RecordProcessOwnership: () => Effect.die("unused"),
	});
}

function make_recovery_runtime(
	database_path: string,
	root: string,
	source_head: string,
	illegal_calls: { execute: number; reconcile: number },
	dispatcher_attempt: DispatcherAttempt,
) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		make_metadata_layer("backend_git_mutation_recovery", "2026-07-14T12:00:31.000Z"),
		JournalNotifierLive,
	);
	const mutation_repository = WorkspaceGitMutationRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const session_repository = WorkspaceGitSessionRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const recovery_registry = make_recovery_registry(root, source_head, illegal_calls);
	const evidence = make_evidence_layer();
	const observer = WorkspaceGitObserverLive.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(recovery_registry),
		Layer.provideMerge(infrastructure),
	);
	const repositories = Layer.merge(mutation_repository, session_repository);
	const support = Layer.mergeAll(NodeCrypto.layer, evidence, observer, recovery_registry);
	const session = WorkspaceGitSessionServiceLive.pipe(
		Layer.provide(Layer.merge(repositories, support)),
	);
	const coordinator_repository = Layer.effect(
		WorkspaceGitMutationRepository,
		Effect.map(WorkspaceGitMutationRepository, (repository) => ({
			...repository,
			MarkExecuting: (approval_id) =>
				repository.MarkExecuting(approval_id).pipe(
					Effect.onExit((exit) =>
						Effect.sync(() => {
							Deferred.doneUnsafe(
								dispatcher_attempt,
								Effect.succeed({ approval_id, exit }),
							);
						}),
					),
				),
		})),
	).pipe(Layer.provide(mutation_repository));
	const coordinator_services = Layer.mergeAll(
		infrastructure,
		coordinator_repository,
		support,
		session,
	);
	const application = WorkspaceGitMutationCoordinatorLive.pipe(
		Layer.provideMerge(coordinator_services),
	);

	return ManagedRuntime.make(application);
}

async function cleanup_harness_once(context: HarnessCleanup) {
	const deadline = Date.now() + cleanup_timeout_ms;
	const failures: Array<unknown> = [];
	const attempt_cleanup = async (description: string, operation: () => PromiseLike<unknown>) => {
		try {
			await operation();
		} catch (cause) {
			failures.push(new Error(`${description} failed`, { cause }));
		}
	};

	if (context.runtime !== undefined) {
		const runtime = context.runtime;

		await attempt_cleanup("Recovery runtime disposal", () =>
			Effect.runPromise(
				runtime.disposeEffect.pipe(Effect.timeout(remaining_timeout_ms(deadline, 10_000))),
			),
		);
	}

	if (context.fixture !== undefined) {
		const fixture = context.fixture;

		await attempt_cleanup("Crash fixture termination", () =>
			terminate_fixture(fixture, remaining_timeout_ms(deadline, 10_000)),
		);
	}

	if (context.socket_path !== undefined) {
		const socket_path = context.socket_path;

		await attempt_cleanup("Git daemon cleanup", () =>
			exit_credential_cache(socket_path, deadline),
		);
		await attempt_cleanup("Git socket cleanup", () =>
			remove_socket(socket_path, context.socket_directory, deadline),
		);
	}

	if (context.socket_directory !== undefined) {
		const socket_directory = context.socket_directory;

		await attempt_cleanup("Git socket directory cleanup", () => {
			assert_harness_socket_directory(socket_directory);

			return remove_harness_path(socket_directory, "Git socket directory cleanup", deadline);
		});
	}

	await attempt_cleanup("Crash harness directory cleanup", async () => {
		assert_harness_directory(context.directory);

		await remove_harness_path(context.directory, "Crash harness directory cleanup", deadline);
	});

	if (failures.length > 0) {
		throw new AggregateError(failures, "Git mutation crash harness cleanup failed");
	}
}

function cleanup_harness(context: HarnessCleanup) {
	if (context.cleanup !== undefined) {
		return context.cleanup;
	}

	let cleanup!: Promise<void>;

	cleanup = (async () => {
		try {
			await cleanup_harness_once(context);
			active_cleanups.delete(context);
		} finally {
			if (context.cleanup === cleanup) {
				delete context.cleanup;
			}
		}
	})();
	context.cleanup = cleanup;

	return cleanup;
}

afterEach(async () => {
	const failures: Array<unknown> = [];

	for (const context of active_cleanups) {
		try {
			await cleanup_harness(context);
		} catch (cause) {
			failures.push(cause);
		}
	}

	if (failures.length > 0) {
		throw new AggregateError(failures, "Git mutation crash afterEach cleanup failed");
	}
}, 120_000);

describe("workspace Git mutation parent-process crash recovery", () => {
	it("quarantines an orphaned production Git execution and retains its exact claim", async () => {
		const { database_path, directory, root, source_head } = await make_repository();
		const credential_id = `crash-proof-${directory.slice(-6)}`;
		const invocation_path = join(directory, "git-invocations.log");
		const cleanup: HarnessCleanup = { directory };

		active_cleanups.add(cleanup);

		try {
			const socket_directory =
				process.platform === "win32"
					? undefined
					: await mkdtemp(join(tmpdir(), socket_directory_prefix));

			if (socket_directory !== undefined) {
				cleanup.socket_directory = socket_directory;
				await chmod(socket_directory, 0o700);
			}

			const socket_path =
				socket_directory === undefined
					? join(tmpdir(), `${socket_path_prefix}${directory.slice(-6)}.sock`)
					: join(socket_directory, "s");

			cleanup.socket_path = socket_path;

			const fixture = spawn(
				process.execPath,
				["--experimental-loader", typescript_loader_url, fixture_path],
				{
					cwd: process.cwd(),
					detached: process.platform !== "win32",
					env: {
						...process.env,
						ARTISAN_GIT_MUTATION_CRASH_DATABASE: database_path,
						ARTISAN_GIT_MUTATION_CRASH_CREDENTIAL: credential_id,
						ARTISAN_GIT_MUTATION_CRASH_INVOCATION: invocation_path,
						ARTISAN_GIT_MUTATION_CRASH_MIGRATIONS: migrations_path,
						ARTISAN_GIT_MUTATION_CRASH_REPOSITORY: root,
						ARTISAN_GIT_MUTATION_CRASH_SOCKET: socket_path,
						ARTISAN_GIT_MUTATION_CRASH_SOURCE_HEAD: source_head,
					},
					stdio: ["ignore", "pipe", "pipe"],
					windowsHide: true,
				},
			);

			cleanup.fixture = fixture;

			await wait_for_readiness(fixture);

			expect(await credential_cache_action(socket_path, "get", credential_id)).toContain(
				`password=${credential_id}`,
			);
			expect((await wait_for_git_processes(socket_path)).length).toBeGreaterThan(0);
			const original_claim = await read_claim_snapshot(database_path);

			kill_parent_process(fixture);
			await wait_for_exit(fixture, "Git mutation crash fixture");

			expect(await credential_cache_action(socket_path, "get", credential_id)).toContain(
				`password=${credential_id}`,
			);
			expect((await wait_for_git_processes(socket_path)).length).toBeGreaterThan(0);

			expect((await readFile(invocation_path, "utf8")).trim().split("\n")).toEqual([
				"execute",
			]);

			const illegal_calls = { execute: 0, reconcile: 0 };
			const dispatcher_attempt = await Effect.runPromise(
				Deferred.make<{
					readonly approval_id: string;
					readonly exit: Exit.Exit<unknown, unknown>;
				}>(),
			);
			const second_approval_id = "workspace_git_mutation:request_second_git_mutation";
			const runtime = make_recovery_runtime(
				database_path,
				root,
				source_head,
				illegal_calls,
				dispatcher_attempt,
			);

			cleanup.runtime = runtime;

			const original = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceGitMutationCoordinator;
					const repository = yield* WorkspaceGitMutationRepository;

					yield* coordinator.Recover;
					yield* coordinator.AwaitIdle;

					return (yield* repository.Query({
						approval_id: original_approval_id,
						thread_id,
					})).approval;
				}),
			);
			const recovered_claims = await read_claims(runtime);
			const recovered_claim = recovered_claims.find(
				({ approval_id }) => approval_id === original_approval_id,
			);
			expect(original).toMatchObject({
				reason: "interrupted",
				state: "outcome_unknown",
			});
			expect(recovered_claim).toEqual(original_claim);
			expect(recovered_claim).toMatchObject({
				execution_completed_at: null,
				execution_started_at: expect.any(String),
				owner_instance_id: "backend_git_mutation_crash",
				workspace_id,
			});
			expect(illegal_calls).toEqual({ execute: 0, reconcile: 0 });
			expect((await readFile(invocation_path, "utf8")).trim().split("\n")).toEqual([
				"execute",
			]);

			const second = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* WorkspaceGitMutationCoordinator;
					const sessions = yield* WorkspaceGitSessionService;

					yield* sessions.Refresh({
						message_id: "refresh_second_git_mutation",
						sent_at: "2026-07-14T12:00:31.000Z",
						thread_id,
						workspace_id,
					});
					const requested = yield* coordinator.Request({
						expected_session_version: 2,
						message_id: "request_second_git_mutation",
						operation: { message: "Second crash-window commit", type: "commit" },
						sent_at: "2026-07-14T12:00:31.000Z",
						thread_id,
						workspace_id,
					});

					const response = yield* coordinator.Respond({
						approval_id: requested.approval.approval_id,
						approved: true,
						message_id: "approve_second_git_mutation",
						sent_at: "2026-07-14T12:00:31.000Z",
						thread_id,
					});
					const dispatch_attempt = yield* Deferred.await(dispatcher_attempt).pipe(
						Effect.timeoutOrElse({
							duration: "10 seconds",
							orElse: () =>
								Effect.fail(
									new Error(
										"Approval response did not wake Git mutation dispatch",
									),
								),
						}),
					);

					return {
						approval_id: requested.approval.approval_id,
						dispatch_attempt,
						response,
					};
				}),
			);

			const dispatcher_failure = Exit.isFailure(second.dispatch_attempt.exit)
				? Cause.findErrorOption(second.dispatch_attempt.exit.cause)
				: Option.none();
			const final_claims = await read_claims(runtime);
			const final_git_processes = await wait_for_git_processes(socket_path);

			expect(second.response.approval.state).toBe("approved");
			expect(second.approval_id).toBe(second_approval_id);
			expect(second.dispatch_attempt.approval_id).toBe(second_approval_id);
			expect(Option.getOrUndefined(dispatcher_failure)).toMatchObject({
				_tag: "WorkspaceGitMutationConflict",
				reason: "claim_conflict",
			});
			expect(final_claims).toEqual([original_claim]);
			expect(illegal_calls).toEqual({ execute: 0, reconcile: 0 });
			expect(final_git_processes.length).toBeGreaterThan(0);
		} finally {
			await cleanup_harness(cleanup);
		}
	}, 120_000);
});
