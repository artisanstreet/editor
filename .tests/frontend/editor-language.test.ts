import { describe, expect, it } from "vitest";
import { Effect, Option } from "effect";

import {
	EditorLanguageForPath,
	EditorLanguageIsHighlighted,
	LoadEditorLanguage,
} from "../../modules/frontend/src/lib/editor/language";

describe("editor language selection", () => {
	it("resolves a grammar from the file extension", () => {
		expect(EditorLanguageForPath("src/lib/editor/service.ts")).toBe("typescript");
		expect(EditorLanguageForPath("src/app.tsx")).toBe("typescript");
		expect(EditorLanguageForPath("src/main.rs")).toBe("rust");
		expect(EditorLanguageForPath("cmd/server/main.go")).toBe("go");
		expect(EditorLanguageForPath("package.json")).toBe("json");
		expect(EditorLanguageForPath("README.md")).toBe("markdown");
		expect(EditorLanguageForPath("styles/global.css")).toBe("css");
		expect(EditorLanguageForPath("pnpm-workspace.yaml")).toBe("yaml");
	});

	/** Svelte files are HTML-shaped, and this repo writes them with the `.sv` extension. */
	it("maps both Svelte extensions onto the HTML grammar", () => {
		expect(EditorLanguageForPath("routes/+page.sv")).toBe("html");
		expect(EditorLanguageForPath("routes/+page.svelte")).toBe("html");
	});

	it("treats a dotfile's whole name as its extension", () => {
		expect(EditorLanguageForPath(".prettierrc")).toBe("json");
		expect(EditorLanguageForPath(".gitignore")).toBe("plaintext");
	});

	/**
	 * The workspace has actually read the file, so a language it declares wins
	 * over the guess from the path — but only when a grammar exists for it.
	 */
	it("prefers a declared language the editor can honour", () => {
		expect(EditorLanguageForPath("scratch/notes", "python")).toBe("python");
		expect(EditorLanguageForPath("scratch/notes.rs", "cobol")).toBe("rust");
		expect(EditorLanguageForPath("scratch/notes", "cobol")).toBe("plaintext");
	});

	it("falls back to plaintext for an unknown extension", () => {
		expect(EditorLanguageForPath("build/output.bin")).toBe("plaintext");
		expect(EditorLanguageForPath("noextension")).toBe("plaintext");
	});

	it("reports whether a language has a grammar to load", () => {
		expect(EditorLanguageIsHighlighted("typescript")).toBe(true);
		expect(EditorLanguageIsHighlighted("plaintext")).toBe(false);
	});

	/** An unhighlighted file is a degraded editor, never a failed one. */
	it("resolves to undefined instead of failing when no grammar applies", async () => {
		const loaded = await Effect.runPromise(LoadEditorLanguage("plaintext"));
		expect(Option.isNone(loaded)).toBe(true);
	});
});
