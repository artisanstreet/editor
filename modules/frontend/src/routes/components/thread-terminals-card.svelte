<script lang="ts" effect>
	/**
	 * The right-rail Terminals card: live terminals for the open thread, each
	 * identified by the command it runs, and a read-only tail viewer behind a
	 * click. Terminals appear when opened and disappear when they exit, the way
	 * finished agents leave the Agents card. Strictly read-only — the viewer
	 * never writes to the PTY.
	 */
	import type { EventEnvelope, TerminalSession } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import { Effect, Exit, Scope, Stream } from "effect";
	import * as Dialog from "$lib/components/ui/dialog";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		ThreadTerminalsController,
		type ThreadTerminalsState,
	} from "$lib/terminal/thread-terminals-controller";
	import {
		append_terminal_output,
		apply_terminal_lifecycle,
		is_live_terminal,
		present_terminal_output,
		terminal_command_line,
		terminal_display_name,
	} from "$lib/terminal/presentation";
	import type { PillHover } from "./hover-pill.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import ThreadTerminals from "./thread-terminals.svelte";

	let {
		hover,
		thread_id,
		workspace_id,
	}: {
		readonly hover: PillHover;
		readonly thread_id: string | undefined;
		readonly workspace_id: string | undefined;
	} = $props();

	const client = yield* ArtisanClient;
	const terminals_controller = yield* ThreadTerminalsController;
	let terminals_state = $state.raw<ThreadTerminalsState | undefined>(
		yield* terminals_controller.Current(thread_id, workspace_id),
	);
	let terminals = $state.raw<ReadonlyArray<TerminalSession>>([]);
	let viewing = $state.raw<TerminalSession | undefined>(undefined);
	let viewer_raw = $state("");
	let output_generation = 0;
	const component_scope = yield* Scope.Scope;
	let output_scope: Scope.Closeable | undefined;

	/**
	 * A terminal's output is a live subscription, not a one-shot read. Keep one
	 * child scope for the selected viewer so replacing or closing it interrupts
	 * the old stream before another one can become current. The component scope
	 * owns every child scope as the teardown backstop.
	 */
	const DetachOutputScope = () => {
		output_generation += 1;
		const previous_scope = output_scope;
		output_scope = undefined;
		return { generation: output_generation, previous_scope };
	};
	const CloseOutputScope = (scope: Scope.Closeable | undefined) =>
		scope === undefined ? Effect.void : Scope.close(scope, Exit.void);

	const ApplyTerminals = (next: ReadonlyMap<string, ThreadTerminalsState>) =>
		Effect.gen(function* () {
			const state = thread_id === undefined ? undefined : next.get(thread_id);
			if (state?.workspace_id !== workspace_id) return;
			terminals_state = state;
			terminals = state._tag === "Ready" ? state.terminals : [];
		});
	yield* terminals_controller.Changes.pipe(Stream.runForEach(ApplyTerminals), Effect.forkScoped);

	/** The thread changing closes the viewer; a reconnect refresh must not. */
	const SelectThread = (
		next_thread_id: string | undefined,
		next_workspace_id: string | undefined,
	) =>
		Effect.gen(function* () {
			const { previous_scope } = DetachOutputScope();
			viewing = undefined;
			viewer_raw = "";
			terminals = [];
			/** Navigation paints immediately; the component scope owns the release. */
			yield* CloseOutputScope(previous_scope).pipe(Effect.forkScoped);
			const current = yield* terminals_controller.Current(next_thread_id, next_workspace_id);
			terminals_state = current;
			if (current?._tag === "Ready") terminals = current.terminals;
			yield* terminals_controller.Refresh(next_thread_id, next_workspace_id).pipe(Effect.forkScoped);
		});

	yield* SelectThread(thread_id, workspace_id);

	const ApplyLifecycle = (envelope: EventEnvelope) =>
		Effect.gen(function* () {
			if (envelope.payload.type !== "terminal.lifecycle") return;
			if (envelope.thread_id !== thread_id) return;
			const terminal = envelope.payload.terminal;
			if (terminal.workspace_id !== workspace_id) return;
			terminals = apply_terminal_lifecycle(terminals, terminal);
		});

	yield* RunAuthoritativeSubscription(
		Effect.gen(function* () {
			return client.Events.pipe(
				Stream.filter((envelope) => envelope.payload.type === "terminal.lifecycle"),
			);
		}),
		ApplyLifecycle,
		() => terminals_controller.Refresh(thread_id, workspace_id),
	).pipe(Effect.forkScoped);

	const live_terminals = $derived(terminals.filter(is_live_terminal));

	const InspectTerminal = (terminal: TerminalSession) =>
		Effect.gen(function* () {
			const { generation, previous_scope } = DetachOutputScope();
			/** A replacement does not retain B until A's stream has fully released. */
			yield* CloseOutputScope(previous_scope);
			if (generation !== output_generation) return;
			viewing = terminal;
			viewer_raw = "";
			const next_output_scope = yield* Scope.fork(component_scope);
			if (generation !== output_generation) {
				yield* Scope.close(next_output_scope, Exit.void);
				return;
			}
			output_scope = next_output_scope;
			yield* Effect.forkIn(FollowOutput(terminal, generation), next_output_scope);
		});

	const HandleViewerOpenChange = (open: boolean) =>
		Effect.gen(function* () {
			if (open) return;
			const { previous_scope } = DetachOutputScope();
			viewing = undefined;
			viewer_raw = "";
			yield* CloseOutputScope(previous_scope);
		});

	/**
	 * Follows the terminal's scrollback: the backend replays the retained
	 * window first, then streams live bytes. When the process exits the stream
	 * ends and the text simply stays readable — the postmortem case. Changing
	 * or closing the selection advances the generation, so an old stream cannot
	 * publish into the current viewer.
	 */
	const FollowOutput = (target: TerminalSession, generation: number) =>
		Effect.scoped(
			Effect.gen(function* () {
				const decoder = yield* Effect.sync(() => new TextDecoder());
				const output = yield* client.OpenTerminalOutput({
					terminal_id: target.terminal_id,
					thread_id: target.thread_id,
					workspace_id: target.workspace_id,
				});
				if (generation !== output_generation) return;

				yield* output.pipe(
					Stream.runForEach((chunk) =>
						Effect.sync(() => {
							if (generation !== output_generation) return;
							viewer_raw = append_terminal_output(
								viewer_raw,
								decoder.decode(chunk, { stream: true }),
							);
						}),
					),
				);
			}),
			).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						/** A dropped stream leaves whatever already arrived on screen. */
					}),
				),
			);

	const presented_output = $derived(present_terminal_output(viewer_raw));
</script>

{#if terminals_state?._tag === "Loading"}
	<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">
		<div class="flex flex-col gap-2 p-3" aria-label="Loading terminals" role="status">
			<div class="h-4 w-3/5 animate-pulse rounded bg-muted"></div>
			<div class="h-4 w-2/5 animate-pulse rounded bg-muted"></div>
		</div>
	</ShaderGlassSurface>
{:else if live_terminals.length > 0}
	<ShaderGlassSurface class="t-resize t-resize-auto min-h-0 max-h-full shrink rounded-xl">
		<div class="flex min-h-0 min-w-0 flex-col p-1">
			<div class="docs-scroll-fade min-h-0 overflow-x-hidden overflow-y-auto">
				<ThreadTerminals entries={live_terminals} {hover} oninspect={InspectTerminal} />
			</div>
		</div>
	</ShaderGlassSurface>
{/if}

<Dialog.Root
	open={viewing !== undefined}
	onOpenChange={yield* HandleViewerOpenChange(event)}
>
	<Dialog.Content class="gap-3 sm:max-w-3xl">
		{#if viewing !== undefined}
			<Dialog.Header class="gap-1">
				<Dialog.Title>{terminal_display_name(viewing)}</Dialog.Title>
				<Dialog.Description class="font-mono text-xs break-all">
					{terminal_command_line(viewing)}
				</Dialog.Description>
			</Dialog.Header>
			<!--
				column-reverse keeps the view pinned to the newest output while the
				process writes, and still lets the reader scroll back — no measuring,
				no scroll bookkeeping.
			-->
			<div
				class="flex h-[60vh] min-h-0 flex-col-reverse overflow-y-auto rounded-xl bg-background/50 ring-1 ring-foreground/10"
			>
				<pre
					class="px-3 py-2.5 font-mono text-xs leading-5 break-words whitespace-pre-wrap text-foreground"><code
						>{presented_output}</code
					></pre>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
