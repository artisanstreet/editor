import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

const ClipboardBoundary = "modules/frontend/src/lib/browser/clipboard.ts";
const Components = [
	"modules/frontend/src/routes/components/conversation-changes-card.sv",
	"modules/frontend/src/routes/components/conversation-turn-footer.sv",
	"modules/frontend/src/routes/components/forge-connection-overlay.sv",
	"modules/frontend/src/routes/debug/emulator/+page.sv",
] as const;

describe("browser clipboard boundary", () => {
	it("owns the foreign Promise API once and models rejection as a tagged failure", () => {
		const source = Read(ClipboardBoundary);

		expect(source).toContain('Data.TaggedError("ClipboardWriteError")');
		expect(source).toContain("Effect.gen(function* ()");
		expect(source).toContain("Effect.tryPromise({");
		expect(source).toContain("catch: (cause) => new ClipboardWriteError({ cause })");
		expect(source).toContain("navigator.clipboard.writeText(text)");
	});

	it.each(Components)("%s uses the shared SER-owned copy operation", (path) => {
		const source = Read(path);

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain('from "$lib/browser/clipboard"');
		expect(source).toContain("WriteClipboardText");
		expect(source).toContain('Effect.catchTag("ClipboardWriteError"');
		expect(source).not.toContain("navigator.clipboard.writeText");
		expect(source).not.toContain("void navigator.clipboard");
	});

	it("keeps copy failures visible at every calling surface", () => {
		expect(Read(Components[0])).toContain("Couldn't copy the path. Try again.");
		expect(Read(Components[1])).toContain("Couldn't copy response. Try again.");
		expect(Read(Components[2])).toContain(
			"Couldn't copy the command. Select it and copy manually.",
		);
		expect(Read(Components[3])).toContain("Could not copy emulator step");
	});

	it("keeps the footer clock as a named yielded generator worker", () => {
		const source = Read(Components[1]);

		expect(source).toContain("const KeepClockCurrent = Effect.gen(function* ()");
		expect(source).toContain("while (true)");
		expect(source).toContain('yield* Effect.sleep("1 second")');
		expect(source).toContain("now = yield* Clock.currentTimeMillis");
		expect(source).toContain("yield* KeepClockCurrent;");
		expect(source).not.toContain("Effect.forever(");
	});
});
