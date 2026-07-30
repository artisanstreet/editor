import { readFileSync } from "node:fs";
import { globSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	MakeConversationRenderBlocks,
	MakeConversationViewState,
} from "../../modules/frontend/src/lib/conversation/store";
import {
	EmulatorSnapshotAt,
	EmulatorStepLabel,
	emulator_scripts,
} from "../../modules/frontend/src/lib/conversation/emulator-scripts";

const workspace = resolve(import.meta.dirname, "../..");

describe("conversation emulator scripts", () => {
	it("defines a distinctly named archetype per provider habit", () => {
		const ids = emulator_scripts.map((script) => script.id);

		expect(ids.length).toBeGreaterThanOrEqual(6);
		expect(new Set(ids).size).toBe(ids.length);
		for (const script of emulator_scripts) {
			expect(script.patches.length).toBeGreaterThan(0);
			expect(script.description).not.toBe("");
		}
	});

	/**
	 * The scrubber's whole premise: every position is a clean rebuild. A script
	 * that violates an invariant halfway would render as a renderer defect on the
	 * page, so it has to fail here instead.
	 */
	it("rebuilds every step of every script without violating an invariant", () => {
		for (const script of emulator_scripts) {
			for (let step = 0; step <= script.patches.length; step += 1) {
				const rebuilt = EmulatorSnapshotAt(script, step);

				expect(rebuilt, `${script.id} step ${step}`).not.toHaveProperty("error");
			}
		}
	});

	it("grows the conversation monotonically as the timeline advances", () => {
		for (const script of emulator_scripts) {
			let entities = 0;

			for (let step = 0; step <= script.patches.length; step += 1) {
				const rebuilt = EmulatorSnapshotAt(script, step);
				if ("error" in rebuilt) throw new Error(`${script.id} failed at ${step}`);

				const next = rebuilt.items.length + rebuilt.turns.length;
				expect(next, `${script.id} step ${step}`).toBeGreaterThanOrEqual(entities);
				entities = next;
			}
		}
	});

	/** Scrubbing backwards must land on the same state as never having advanced. */
	it("reads the same snapshot at a step regardless of how it was reached", () => {
		for (const script of emulator_scripts) {
			const midpoint = Math.floor(script.patches.length / 2);

			expect(EmulatorSnapshotAt(script, midpoint)).toEqual(
				EmulatorSnapshotAt(script, midpoint),
			);
		}
	});

	it("clamps a position outside the script to its bounds", () => {
		const script = emulator_scripts[0]!;

		expect(EmulatorSnapshotAt(script, -5)).toEqual(EmulatorSnapshotAt(script, 0));
		expect(EmulatorSnapshotAt(script, script.patches.length + 10)).toEqual(
			EmulatorSnapshotAt(script, script.patches.length),
		);
	});

	it("names every step without counting", () => {
		for (const script of emulator_scripts) {
			expect(EmulatorStepLabel(script, 0)).toBe("Empty conversation");
			for (let step = 1; step <= script.patches.length; step += 1) {
				expect(EmulatorStepLabel(script, step)).not.toBe("");
			}
		}
	});

	/**
	 * The archetypes exist to drive the renderer, so each completed script must
	 * actually produce blocks rather than merely decode.
	 */
	it("renders blocks for the completed form of every script", () => {
		for (const script of emulator_scripts) {
			const rebuilt = EmulatorSnapshotAt(script, script.patches.length);
			if ("error" in rebuilt) throw new Error(`${script.id} failed to rebuild`);

			const view = MakeConversationViewState(rebuilt);

			expect(view._tag, script.id).toBe("applied");

			const blocks = MakeConversationRenderBlocks(view.state);

			expect(blocks.length, script.id).toBeGreaterThan(0);
		}
	});

	/** The two habits the emulator exists for must differ in what the trace can show. */
	it("separates a hidden-reasoning shape from a visible-reasoning one", () => {
		const reasoning_count = (id: string) => {
			const script = emulator_scripts.find((candidate) => candidate.id === id)!;
			const rebuilt = EmulatorSnapshotAt(script, script.patches.length);
			if ("error" in rebuilt) throw new Error(`${id} failed to rebuild`);

			return rebuilt.items.filter((item) => item.type === "reasoning_summary").length;
		};

		expect(reasoning_count("hidden-reasoning")).toBe(0);
		expect(reasoning_count("visible-reasoning")).toBeGreaterThan(0);
	});
});

describe("emulator production gate", () => {
	/** The page's own title and one script id: markup and data are stubbed separately. */
	const markers = ["Conversation emulator", "hidden-reasoning"];

	it("stubs both the page and its scripts in a production build", () => {
		const config = readFileSync(resolve(workspace, "modules/frontend/vite.config.ts"), "utf8");

		expect(config).toContain("development_only_surfaces");
		expect(config).toContain("/routes/debug/emulator/+page.sv");
		expect(config).toContain("/lib/conversation/emulator-scripts.ts");
	});

	it("guards the page body on the development flag", () => {
		const page = readFileSync(
			resolve(workspace, "modules/frontend/src/routes/debug/emulator/+page.sv"),
			"utf8",
		);

		expect(page).toContain('import { dev } from "$app/environment"');
		expect(page).toContain("{#if !dev}");
	});

	/**
	 * Mirrors the development-origin gate: the guard is only a claim until the
	 * built bundle is searched for what it was supposed to drop.
	 */
	it("keeps the emulator surface out of the production bundle", () => {
		const bundle = globSync("**/*.js", {
			cwd: resolve(workspace, ".dist/frontend"),
		});

		expect(bundle.length, "build .dist/frontend before running this test").toBeGreaterThan(0);

		const leaking = bundle.filter((file) => {
			const contents = readFileSync(resolve(workspace, ".dist/frontend", file), "utf8");
			return markers.some((marker) => contents.includes(marker));
		});

		expect(leaking).toEqual([]);
	});
});
