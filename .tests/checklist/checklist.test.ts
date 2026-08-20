import { describe, expect, it } from "vitest";

import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";

import {
	apply_checklist_event,
	create_checklist_state,
	summarize_checklist,
	type ChecklistEvent,
	type ChecklistSummary,
} from "../../modules/checklist/src/model.ts";
import { resolve_presentation } from "../../modules/checklist/src/presentation.ts";
import { ScheduleChecklist } from "../../modules/checklist/src/scheduler.ts";
import { command, value, type Step } from "../../modules/checklist/src/step.ts";

const NodeProcessLive = NodeChildProcessSpawner.layer.pipe(
	Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer)),
);

interface RunResult {
	readonly events: ReadonlyArray<ChecklistEvent>;
	readonly passed: boolean;
	readonly summary: ChecklistSummary;
}

const run_checklist = async (
	steps: ReadonlyArray<Step>,
	options: { readonly concurrency?: "unbounded" | number } = {},
): Promise<RunResult> => {
	const events: ChecklistEvent[] = [];
	let state = create_checklist_state();

	const passed = await Effect.runPromise(
		ScheduleChecklist({
			emit: (event) => {
				events.push(event);
				state = apply_checklist_event(state, event);
			},
			options: { ...options, title: "test" },
			steps,
		}).pipe(Effect.scoped, Effect.provide(NodeProcessLive)),
	);

	return { events, passed, summary: summarize_checklist(state) };
};

const status_of = (summary: ChecklistSummary, name: string) =>
	summary.steps.find((entry) => entry.name === name)?.status;

describe("checklist scheduling", () => {
	it("runs an array in declaration order", async () => {
		const order: string[] = [];
		const result = await run_checklist([
			{ name: "first", run: () => void order.push("first") },
			{ name: "second", run: () => void order.push("second") },
			{ name: "third", run: () => void order.push("third") },
		]);

		expect(order).toEqual(["first", "second", "third"]);
		expect(result.passed).toBe(true);
		expect(result.summary.outcome).toBe("passed");
	});

	it("threads a declared value from producer to consumer", async () => {
		const version = value<string>("version");
		let observed: string | undefined;

		const result = await run_checklist([
			{ name: "produce", provides: version, run: () => "1.2.3" },
			{ name: "consume", run: (step) => void (observed = step.get(version)) },
		]);

		expect(observed).toBe("1.2.3");
		expect(result.passed).toBe(true);
	});

	it("fails the step that reads a value no completed step produced", async () => {
		const missing = value<string>("missing");
		const result = await run_checklist([{ name: "consume", run: (step) => step.get(missing) }]);

		expect(result.passed).toBe(false);
		expect(status_of(result.summary, "consume")).toBe("failed");
		expect(result.summary.steps[0]?.failure).toContain("missing");
	});

	it("aborts on failure and reports the untouched steps as cancelled", async () => {
		let reached = false;
		const result = await run_checklist([
			{ name: "ok", run: () => undefined },
			{
				name: "boom",
				run: () => {
					throw new Error("exploded");
				},
			},
			{ name: "never", run: () => void (reached = true) },
		]);

		expect(reached).toBe(false);
		expect(result.passed).toBe(false);
		expect(status_of(result.summary, "ok")).toBe("passed");
		expect(status_of(result.summary, "boom")).toBe("failed");
		expect(status_of(result.summary, "never")).toBe("cancelled");
	});

	it("records an optional failure without ending the run", async () => {
		let reached = false;
		const result = await run_checklist([
			{
				name: "flaky",
				optional: true,
				run: () => {
					throw new Error("ignored");
				},
			},
			{ name: "after", run: () => void (reached = true) },
		]);

		expect(reached).toBe(true);
		expect(result.passed).toBe(true);
		expect(status_of(result.summary, "flaky")).toBe("failed");
	});

	it("skips a whole subtree when a group's condition is false", async () => {
		let reached = false;
		const result = await run_checklist([
			{
				name: "group",
				steps: [{ name: "child", run: () => void (reached = true) }],
				when: () => false,
			},
		]);

		expect(reached).toBe(false);
		expect(status_of(result.summary, "child")).toBe("skipped");
		expect(result.passed).toBe(true);
	});

	it("retries a failing step up to the configured attempts", async () => {
		let attempts = 0;
		const result = await run_checklist([
			{
				name: "eventually",
				retry: 3,
				run: () => {
					attempts += 1;
					if (attempts < 3) throw new Error("not yet");
				},
			},
		]);

		expect(attempts).toBe(3);
		expect(result.passed).toBe(true);
	});
});

describe("checklist concurrency", () => {
	it("overlaps a concurrent group's children", async () => {
		let peak = 0;
		let active = 0;
		const hold = () =>
			new Promise<void>((resolve) => {
				active += 1;
				peak = Math.max(peak, active);
				setTimeout(() => {
					active -= 1;
					resolve();
				}, 30);
			});

		await run_checklist([
			{
				concurrency: "unbounded",
				name: "fan",
				steps: [
					{ name: "a", run: hold },
					{ name: "b", run: hold },
					{ name: "c", run: hold },
				],
			},
		]);

		expect(peak).toBeGreaterThan(1);
	});

	it("hides a concurrent sibling's value until the group completes", async () => {
		const token = value<string>("token");
		let sibling_saw: string | undefined = "unset";
		let after_saw: string | undefined;

		await run_checklist([
			{
				concurrency: "unbounded",
				name: "fan",
				steps: [
					{
						name: "producer",
						provides: token,
						run: () => "produced",
					},
					{
						name: "sibling",
						// Runs alongside the producer, so the value must not be visible.
						run: (step) => void (sibling_saw = step.peek(token)),
					},
				],
			},
			{ name: "after", run: (step) => void (after_saw = step.get(token)) },
		]);

		expect(sibling_saw).toBeUndefined();
		expect(after_saw).toBe("produced");
	});
});

describe("checklist dynamic groups", () => {
	it("expands a group whose children come from an upstream value", async () => {
		const items = value<ReadonlyArray<string>>("items");
		const uploaded: string[] = [];

		const result = await run_checklist([
			{ name: "discover", provides: items, run: () => ["alpha", "beta"] },
			{
				name: "upload",
				steps: (step) =>
					step.get(items).map((item) => ({
						name: `upload ${item}`,
						run: () => void uploaded.push(item),
					})),
			},
		]);

		expect(uploaded).toEqual(["alpha", "beta"]);
		expect(result.events.some((event) => event.type === "expand")).toBe(true);
		expect(status_of(result.summary, "upload alpha")).toBe("passed");
	});
});

describe("checklist commands", () => {
	it("captures argv-form output and passes on a zero exit", async () => {
		const result = await run_checklist([
			{ name: "argv", run: ["node", "-e", "console.log('from argv')"] },
		]);

		expect(result.passed).toBe(true);
		expect(
			result.events.some((event) => event.type === "log" && event.line.includes("from argv")),
		).toBe(true);
	});

	it("runs a command built from an upstream value", async () => {
		const message = value<string>("message");
		const result = await run_checklist([
			{ name: "produce", provides: message, run: () => "built dynamically" },
			{
				name: "echo",
				run: (step) => command(["node", "-e", `console.log("${step.get(message)}")`]),
			},
		]);

		expect(result.passed).toBe(true);
		expect(
			result.events.some(
				(event) => event.type === "log" && event.line.includes("built dynamically"),
			),
		).toBe(true);
	});

	it("stores a returned array as a value rather than executing it", async () => {
		const listing = value<ReadonlyArray<string>>("listing");
		let observed: ReadonlyArray<string> | undefined;

		const result = await run_checklist([
			{ name: "produce", provides: listing, run: () => ["node", "--version"] },
			{ name: "consume", run: (step) => void (observed = step.get(listing)) },
		]);

		expect(observed).toEqual(["node", "--version"]);
		expect(result.passed).toBe(true);
	});

	it("fails the step on a non-zero exit", async () => {
		const result = await run_checklist([
			{ name: "exits", run: ["node", "-e", "process.exit(4)"] },
		]);

		expect(result.passed).toBe(false);
		expect(result.summary.steps[0]?.failure).toContain("4");
	});
});

describe("presentation resolution", () => {
	const base = {
		argv: [] as ReadonlyArray<string>,
		environment: {} as Readonly<Record<string, string | undefined>>,
		requested: undefined,
		stdout_is_tty: true,
	};

	it("uses the dashboard on an interactive terminal", () => {
		expect(resolve_presentation(base)).toBe("tui");
	});

	it("honours --no-tui above every other signal", () => {
		expect(resolve_presentation({ ...base, argv: ["--no-tui"], requested: "tui" })).toBe(
			"plain",
		);
	});

	it("honours --json and --plain", () => {
		expect(resolve_presentation({ ...base, argv: ["--json"] })).toBe("json");
		expect(resolve_presentation({ ...base, argv: ["--plain"] })).toBe("plain");
	});

	it("falls back to plain text off a terminal, in CI, and when NO_TUI is set", () => {
		expect(resolve_presentation({ ...base, stdout_is_tty: false })).toBe("plain");
		expect(resolve_presentation({ ...base, environment: { CI: "1" } })).toBe("plain");
		expect(resolve_presentation({ ...base, environment: { NO_TUI: "1" } })).toBe("plain");
		expect(resolve_presentation({ ...base, environment: { TERM: "dumb" } })).toBe("plain");
	});

	it("refuses the dashboard without a terminal even when asked for it", () => {
		expect(resolve_presentation({ ...base, argv: ["--tui"], stdout_is_tty: false })).toBe(
			"plain",
		);
		expect(resolve_presentation({ ...base, requested: "tui", stdout_is_tty: undefined })).toBe(
			"plain",
		);
		/** json is a data sink, not a renderer, so it is unaffected by the terminal. */
		expect(resolve_presentation({ ...base, argv: ["--json"], stdout_is_tty: false })).toBe(
			"json",
		);
	});
});
