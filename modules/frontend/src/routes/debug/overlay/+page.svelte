<script lang="ts" effect>
	import { dev } from "$app/environment";
	import {
		ArtisanClientError,
		type TransportDiagnosticsSnapshot,
	} from "@artisan/transport/client";
	import PlayerTrackNext from "@tabler/icons-svelte/icons/player-track-next";
	import PlayerTrackPrev from "@tabler/icons-svelte/icons/player-track-prev";
	import { Effect } from "effect";
	import { Button } from "$lib/components/ui/button";
	import {
		BeginForgeHydration,
		FailForgeHydration,
		ObserveForgeConnection,
		type ForgeGateModel,
	} from "$lib/forge/gate";
	import ForgeConnectionOverlay from "$/components/forge-connection-overlay.svelte";

	interface OverlayScenario {
		readonly id: string;
		readonly label: string;
		readonly model: ForgeGateModel;
	}

	const base: ForgeGateModel = {
		dismissed: false,
		has_hydrated_shell: false,
		hydration_generation: 0,
		state: { phase: "connecting" },
	};
	const connection_error = new ArtisanClientError({
		cause: new Error("connection refused"),
		code: "connection",
		message: "WebSocket handshake failed: connection refused",
		protocol_code: "transport.connection",
		retryable: true,
	});
	const protocol_error = new ArtisanClientError({
		cause: new Error("protocol version mismatch"),
		code: "protocol",
		message: "Artisan and Forge use incompatible protocol versions.",
		protocol_code: "transport.version_mismatch",
		retryable: false,
	});
	const hydration_base = BeginForgeHydration({ ...base, has_hydrated_shell: true });
	/** Representative journal so the debug surface exercises the activity list. */
	const fixture_journal: TransportDiagnosticsSnapshot = {
		dropped: 3,
		events: [
			{ at: "2026-08-06T09:12:01.000Z", kind: "session.attempt", ordinal: 4 },
			{
				at: "2026-08-06T09:12:01.180Z",
				connection_id: "connection_debug",
				event_cursor_count: 6,
				journal_sequence: 384,
				kind: "session.negotiating",
				resume_mode: "resume",
			},
			{
				at: "2026-08-06T09:12:01.410Z",
				connection_id: "connection_debug",
				kind: "session.ready",
				negotiation_ms: 410,
			},
			{
				at: "2026-08-06T09:17:44.902Z",
				code: "connection",
				detail: "socket closed by peer",
				kind: "session.ended",
				lifetime_ms: 343_492,
				message: "The transport session disconnected.",
				protocol_code: "",
				reached_ready: true,
			},
			{
				at: "2026-08-06T09:17:45.630Z",
				attempts: 3,
				code: "connection",
				kind: "supervisor.exhausted",
				message: "WebSocket handshake failed: connection refused",
				protocol_code: "transport.connection",
			},
		],
	};
	const read_diagnostics = Effect.gen(function* () {
		return fixture_journal;
	});
	/**
	 * One entry per gate phase the real overlay can present, with
	 * representative data. The "ready" phase is deliberately absent: it
	 * renders nothing, so cycling into it would only look like a broken page.
	 * The unpaired and other-instances variants of the error states depend on
	 * live discovery against this machine and cannot be forced from here.
	 */
	const scenarios: ReadonlyArray<OverlayScenario> = [
		{
			id: "connecting",
			label: "Connecting",
			model: { ...base, state: { phase: "connecting" } },
		},
		{
			id: "reconnecting",
			label: "Reconnecting",
			model: { ...base, has_hydrated_shell: true, state: { phase: "reconnecting" } },
		},
		{
			id: "hydrating",
			label: "Hydrating",
			model: {
				...base,
				hydration_generation: 1,
				state: { generation: 1, phase: "hydrating" },
			},
		},
		{
			id: "exhausted",
			label: "Offline",
			model: ObserveForgeConnection(base, {
				attempts: 6,
				error: connection_error,
				phase: "exhausted",
			}),
		},
		{
			id: "protocol-failed",
			label: "Protocol failed",
			model: ObserveForgeConnection(base, {
				attempts: 1,
				error: protocol_error,
				phase: "exhausted",
			}),
		},
		{
			id: "hydration-failed",
			label: "Threads failed",
			model: FailForgeHydration(
				hydration_base,
				hydration_base.hydration_generation,
				"threads",
				protocol_error,
			),
		},
		{
			id: "defaults-failed",
			label: "Defaults failed",
			model: FailForgeHydration(
				hydration_base,
				hydration_base.hydration_generation,
				"session_defaults",
				protocol_error,
			),
		},
	];

	let index = $state(0);
	let dismissed = $state(false);
	const scenario = $derived(scenarios[index]);
	const model = $derived(
		scenario === undefined ? undefined : { ...scenario.model, dismissed },
	);

	const Cycle = (delta: number) =>
		Effect.gen(function* () {
			index = (index + delta + scenarios.length) % scenarios.length;
			dismissed = false;
		});
	const JumpTo = (id: string) =>
		Effect.gen(function* () {
			const next = scenarios.findIndex((candidate) => candidate.id === id);
			if (next < 0) return;
			index = next;
			dismissed = false;
		});
	const Dismiss = () =>
		Effect.gen(function* () {
			dismissed = true;
		});

	/**
	 * The retry affordances jump to the progress state a real retry would
	 * enter, so clicking through the overlay behaves like the flow it mocks.
	 */
	const retry_connection = Effect.gen(function* () {
		yield* JumpTo("connecting");
	});
	const retry_hydration = Effect.gen(function* () {
		yield* JumpTo("hydrating");
	});
</script>

<!--
	Unguarded because it cannot be otherwise: `svelte:head` may not sit inside a
	block. The production build stubs this whole file, so the name never ships.
-->
<svelte:head><title>Forge overlay states</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else if scenario !== undefined && model !== undefined}
	<!-- Fixed so the production overlay's absolute inset fills the viewport, panel chrome and all. -->
	<div class="fixed inset-0 z-40">
			<ForgeConnectionOverlay
				{model}
				ondismiss={Dismiss}
			{read_diagnostics}
			{retry_connection}
			{retry_hydration}
		/>
	</div>

	<div
		class="card fixed top-4 right-4 z-[60] flex items-center gap-1 rounded-full bg-linear-to-b from-surface-225 to-surface-200 p-1 dark:from-surface-800 dark:to-surface-925"
	>
		<Button variant="ghost" size="icon-sm" aria-label="Previous status" onclick={yield* Cycle(-1)}>
			<PlayerTrackPrev />
		</Button>
		<span class="min-w-44 text-center text-xs text-foreground">
			{dismissed ? `${scenario.label} · dismissed` : scenario.label}
			<span class="text-muted-foreground tabular-nums">· {index + 1}/{scenarios.length}</span>
		</span>
		<Button variant="ghost" size="icon-sm" aria-label="Next status" onclick={yield* Cycle(1)}>
			<PlayerTrackNext />
		</Button>
	</div>
{/if}
