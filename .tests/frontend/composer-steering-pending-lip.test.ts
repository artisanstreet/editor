import { readFileSync } from "node:fs";
import { Cause, Effect, Exit, Fiber } from "effect";
import { describe, expect, it } from "vitest";
import {
	begin_pending_steering_lip,
	release_pending_steering_lip,
	type SteeringPendingLipState,
} from "../../modules/frontend/src/lib/thread-interaction/steering-pending-lip";
import { MakeSteeringStages } from "../../modules/frontend/src/lib/thread-interaction/steering-stages";

const Read = (path: string) => readFileSync(path, "utf8");

describe("composer steering acknowledgement lip", () => {
	it("keeps a submitted steer outside the transcript and above the stable shader surface", () => {
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const stages = Read("modules/frontend/src/lib/thread-interaction/steering-stages.ts");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(composer).toContain('import { LipCard } from "$lib/components/ui/lip-card";');
		expect(composer).toContain("if (show_pending_lip && run_active) {");
		expect(composer).toContain(
			"pending_lip_generation = steering.Begin(submission, Date.now());",
		);
		expect(composer.indexOf("const submission: ComposerSubmission")).toBeLessThan(
			composer.indexOf("pending_lip_generation = steering.Begin("),
		);
		expect(composer).toContain("yield* deliver(submission).pipe(");
		expect(composer).toContain(
			"group/lip prose-column pointer-events-auto w-full max-w-(--prose-width) flex-col-reverse",
		);
		expect(composer).toContain("data-[open=false]:overflow-visible");
		expect(composer).toContain("group-data-[open=true]/lip:rounded-t-none");
		expect(composer.indexOf("<LipCard")).toBeLessThan(
			composer.indexOf('<ShaderGlassSurface\n\t\t\tclass="t-resize'),
		);
		expect(composer).toContain("open={pending_lip_state.pending.length > 0}");
		/** The editor stays live under the stack; queueing must not blind the next steer. */
		expect(composer).not.toContain("class:invisible");
		expect(composer).toContain("pending_steer.submission.text.trim()");
		expect(stages).toContain(
			"const remaining_ms = minimum_lip_display_ms - (Date.now() - pending_lip.started_at);",
		);
		expect(workspace).toContain("onsteeringchange={SetSteeringPending}");
		expect(route).toContain("AwaitSteeringAcknowledged(receipt.command_id)");
		expect(route).toContain("ResolveAcceptedProjectionWaiters(snapshot)");
		expect(route).not.toContain("AwaitAcceptedProjection");
		expect(route).not.toContain("CurrentSnapshot");
	});

	/**
	 * A queued steer and an in-flight steer are different states with different
	 * surfaces. The lip alone says "queued"; the "Steering" label may only rise
	 * once Artisan projects the canonical message, and it lowers when the engine
	 * acknowledges — raising it at submit put both on screen at once.
	 */
	it("stages the queued lip before the label and the label before acknowledgement", () => {
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const stages = Read("modules/frontend/src/lib/thread-interaction/steering-stages.ts");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		const queued_lip_begin = composer.slice(
			composer.indexOf("if (show_pending_lip && run_active) {"),
			composer.indexOf("let retain_pending_lip"),
		);
		expect(queued_lip_begin).toContain("steering.Begin(");
		expect(queued_lip_begin).not.toContain("onsteeringchange");
		expect(queued_lip_begin).not.toContain("TakeUp");
		expect(stages.indexOf("harness.SteeringChanged(true,")).toBeGreaterThan(
			stages.indexOf("TakeUp: (generation: number) =>"),
		);
		expect(stages.indexOf("harness.SteeringChanged(false)")).toBeGreaterThan(
			stages.indexOf("Settle: (generation: number) =>"),
		);
		expect(composer.indexOf("yield* steering_echo.pipe(")).toBeLessThan(
			composer.indexOf("Effect.andThen(steering.TakeUp(steer_generation))"),
		);
		expect(composer.indexOf("Effect.andThen(steering.TakeUp(steer_generation))")).toBeLessThan(
			composer.indexOf("Effect.andThen(steering_settlement)"),
		);
		expect(route).toContain("steering_echo: AwaitCanonicalUserMessage(receipt.command_id)");
		/** A swallowed outcome starves the composer of both settlement effects. */
		expect(workspace).toContain("return yield* submit(submission).pipe(");
	});

	/** Overlapping steers stack; each settlement releases only its own row. */
	it("releases each steer's own row and leaves the rest of the stack standing", () => {
		let state: SteeringPendingLipState<string> = { next_generation: 0, pending: [] };
		const steer_a = begin_pending_steering_lip(state, "steer A", 100);
		state = steer_a.state;
		const steer_b = begin_pending_steering_lip(state, "steer B", 200);
		state = steer_b.state;
		/** Newest first: the stack renders steer B above the still-waiting steer A. */
		expect(state.pending.map((lip) => lip.submission)).toEqual(["steer B", "steer A"]);

		const release_a = release_pending_steering_lip(state, steer_a.begun.generation);
		expect(release_a.released).toBe(true);
		expect(release_a.state.pending.map((lip) => lip.submission)).toEqual(["steer B"]);

		/** A settlement that already released holds no authority over the survivor. */
		const stale = release_pending_steering_lip(release_a.state, steer_a.begun.generation);
		expect(stale.released).toBe(false);
		expect(stale.state.pending.map((lip) => lip.submission)).toEqual(["steer B"]);

		const release_b = release_pending_steering_lip(stale.state, steer_b.begun.generation);
		expect(release_b.released).toBe(true);
		expect(release_b.state.pending).toEqual([]);
	});

	/** A late settlement of steer A must not lower the label steer B raised. */
	it("hands the label between overlapping steers by generation", async () => {
		let lip: SteeringPendingLipState<string> = { next_generation: 0, pending: [] };
		const changes: Array<{ readonly pending: boolean; readonly source_reference?: string }> =
			[];
		const stages = MakeSteeringStages<string>({
			Lip: () => lip,
			ReplaceLip: (next) => {
				lip = next;
			},
			SteeringChanged: (pending, source_reference) =>
				changes.push({ pending, source_reference }),
			Withdraw: () => Effect.void,
		});

		/** Queueing alone never announces steering. */
		const steer_a = stages.Begin("steer A", 0);
		expect(lip.pending.map((row) => row.submission)).toEqual(["steer A"]);
		expect(changes).toEqual([]);

		/** The echo releases only this steer's row and raises the label in one act. */
		expect(stages.Bind(steer_a, "command-a")).toBe(false);
		await Effect.runPromise(stages.TakeUp(steer_a));
		expect(lip.pending).toEqual([]);
		expect(changes).toEqual([{ pending: true, source_reference: "command-a" }]);

		const steer_b = stages.Begin("steer B", 0);
		expect(stages.Bind(steer_b, "command-b")).toBe(false);
		await Effect.runPromise(stages.TakeUp(steer_b));
		expect(lip.pending).toEqual([]);
		expect(changes).toEqual([
			{ pending: true, source_reference: "command-a" },
			{ pending: true, source_reference: "command-b" },
		]);

		/** Steer A settles late; steer B's label survives it. */
		await Effect.runPromise(stages.Settle(steer_a));
		expect(changes).toEqual([
			{ pending: true, source_reference: "command-a" },
			{ pending: true, source_reference: "command-b" },
		]);

		await Effect.runPromise(stages.Settle(steer_b));
		expect(changes).toEqual([
			{ pending: true, source_reference: "command-a" },
			{ pending: true, source_reference: "command-b" },
			{ pending: false, source_reference: undefined },
		]);
	});

	/** A recall acts on intent immediately and completes durably once the send has a name. */
	it("withdraws a queued steer before and after its send is named", async () => {
		let lip: SteeringPendingLipState<string> = { next_generation: 0, pending: [] };
		const withdrawn: Array<string> = [];
		const changes: Array<boolean> = [];
		const stages = MakeSteeringStages<string>({
			Lip: () => lip,
			ReplaceLip: (next) => {
				lip = next;
			},
			SteeringChanged: (pending) => changes.push(pending),
			Withdraw: (command_id) =>
				Effect.sync(() => {
					withdrawn.push(command_id);
				}),
		});

		/** Nameless: the lip row closes now, and `Bind` reports the recall to complete. */
		const nameless = stages.Begin("steer A", 0);
		expect(await Effect.runPromise(stages.Withdraw(nameless))).toBe(true);
		expect(lip.pending).toEqual([]);
		expect(withdrawn).toEqual([]);
		expect(stages.Bind(nameless, "command-a")).toBe(true);

		/** Named: the recall runs durably and interrupts the settlement watcher. */
		const named = stages.Begin("steer B", 0);
		expect(stages.Bind(named, "command-b")).toBe(false);
		const settlement = Effect.runFork(Effect.never);
		stages.Adopt(named, settlement);
		expect(await Effect.runPromise(stages.Withdraw(named))).toBe(true);
		expect(withdrawn).toEqual(["command-b"]);
		expect(lip.pending).toEqual([]);
		const settlement_exit = await Effect.runPromise(Fiber.await(settlement));
		expect(
			Exit.isFailure(settlement_exit) && Cause.hasInterruptsOnly(settlement_exit.cause),
		).toBe(true);
		/** The label never spoke for either recalled steer. */
		expect(changes).toEqual([]);
	});

	/**
	 * The queued rows stack newest-on-top, each one line that elides, each
	 * offering the queued state's two actions — recall the message into the
	 * editor, or discard it — and each growing the lip open on the accordion's
	 * own timing instead of snapping it taller.
	 */
	it("stacks queued steers newest first with elided rows that grow the lip", () => {
		const actions = Read("modules/frontend/src/routes/components/composer/queued-steer.ts");
		const animations = Read("modules/frontend/src/lib/styles/animations.css");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");
		const lip_row = Read("modules/frontend/src/routes/components/composer/steering-lip.svelte");
		const lip_state = Read(
			"modules/frontend/src/lib/thread-interaction/steering-pending-lip.ts",
		);
		const route = Read("modules/frontend/src/routes/components/thread-route.svelte");
		const utilities = Read("modules/frontend/src/lib/styles/utilities.css");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		/** Newest first is the state's own order, not a render-time sort. */
		expect(lip_state).toContain("pending: [begun, ...state.pending],");
		expect(composer).toContain(
			"{#each pending_lip_state.pending as pending_steer (pending_steer.generation)}",
		);
		expect(composer).toContain('<div class="t-lip-row">');
		expect(composer).toContain('<div class="t-lip-row-inner">');
		expect(utilities).toContain("@utility t-lip-row");
		expect(utilities).toContain("animation: lip-row-grow var(--acc-expand) var(--acc-ease);");
		expect(animations).toContain("@keyframes lip-row-grow");
		expect(animations.slice(animations.indexOf("@keyframes lip-row-grow"))).toContain(
			"grid-template-rows: 0fr;",
		);
		expect(lip_row).toContain("min-w-0 flex-1 truncate");
		expect(lip_row).toContain('from "@tabler/icons-svelte/icons/pencil"');
		expect(lip_row).toContain('from "@tabler/icons-svelte/icons/trash"');
		expect(lip_row).toContain('aria-label="Edit queued message"');
		expect(lip_row).toContain('aria-label="Discard queued message"');
		expect(composer).toContain("editable={onwithdraw !== undefined}");
		expect(composer).toContain("ondiscard={queued_steer.Discard(pending_steer)}");
		expect(composer).toContain("onedit={queued_steer.Edit(pending_steer)}");
		/** An edit restores the composed text only after the recall actually holds. */
		expect(actions).toContain("if (yield* TryWithdraw(row.generation))");
		expect(route).toContain('type: "thread.withdraw_message"');
		expect(route).toContain("onwithdraw={WithdrawQueuedMessage}");
		expect(workspace).toContain("{onwithdraw}");
	});
});
