import { describe, expect, it } from "vitest";

import { ShortProjectPath } from "../../modules/frontend/src/lib/root/project-path";

describe("short project path", () => {
	it("collapses a Windows home directory and drops the repeated project name", () => {
		expect(
			ShortProjectPath("C:\\Users\\sander\\Desktop\\artisan-editor", "artisan-editor"),
		).toBe("~\\Desktop");
	});

	it("collapses a POSIX home directory", () => {
		expect(ShortProjectPath("/home/sander/code/artisan-editor", "artisan-editor")).toBe(
			"~/code",
		);
	});

	it("collapses a macOS home directory", () => {
		expect(ShortProjectPath("/Users/sander/code/artisan", "artisan")).toBe("~/code");
	});

	it("keeps paths outside a home directory", () => {
		expect(ShortProjectPath("/srv/checkouts/artisan", "artisan")).toBe("/srv/checkouts");
	});

	it("keeps a trailing segment that is not the project name", () => {
		expect(ShortProjectPath("C:\\Users\\sander\\Desktop\\artisan-editor", "Artisan")).toBe(
			"~\\Desktop\\artisan-editor",
		);
	});

	it("retains native separators without a home directory to collapse", () => {
		expect(ShortProjectPath("D:\\work\\repos\\artisan", "artisan")).toBe("D:\\work\\repos");
	});

	it("keeps an ordinary UNC server and share", () => {
		expect(ShortProjectPath("\\\\server\\share\\team\\artisan", "artisan")).toBe(
			"\\\\server\\share\\team",
		);
	});

	it("keeps a WSL distribution while collapsing its Linux home directory", () => {
		expect(
			ShortProjectPath("\\\\wsl.localhost\\Ubuntu\\home\\sander\\code\\artisan", "artisan"),
		).toBe("~\\code · Ubuntu (WSL)");
	});

	it("recognises WSL's legacy UNC hostname too", () => {
		expect(ShortProjectPath("\\\\wsl$\\Ubuntu\\home\\sander\\code\\artisan", "artisan")).toBe(
			"~\\code · Ubuntu (WSL)",
		);
	});

	it("keeps a home marker when the project sits directly in the home directory", () => {
		expect(ShortProjectPath("/home/sander/artisan", "artisan")).toBe("~/");
	});

	it("ignores a trailing separator", () => {
		expect(ShortProjectPath("/home/sander/code/artisan/", "artisan")).toBe("~/code");
	});

	it("does not mistake a home-named directory elsewhere for the home root", () => {
		expect(ShortProjectPath("/srv/home/sander/artisan", "artisan")).toBe("/srv/home/sander");
	});

	it("keeps the broad and nearest context for a deep home path", () => {
		expect(
			ShortProjectPath(
				"C:\\Users\\sander\\Development\\clients\\northwind\\repositories\\artisan",
				"artisan",
			),
		).toBe("~\\Development\\…\\repositories");
	});

	it("keeps project folder context when its name differs from the display name", () => {
		expect(ShortProjectPath("C:\\Users\\sander\\Desktop\\artisan-editor", "Artisan")).toBe(
			"~\\Desktop\\artisan-editor",
		);
	});

	it("matches a repeated Windows project name without case sensitivity", () => {
		expect(ShortProjectPath("C:\\Users\\Sander\\Desktop\\Artisan", "artisan")).toBe(
			"~\\Desktop",
		);
	});

	it("honours an explicit display separator independently of the source path", () => {
		expect(
			ShortProjectPath(
				"C:\\Users\\sander\\Desktop\\artisan-editor",
				"artisan-editor",
				"forward-slash",
			),
		).toBe("~/Desktop");
		expect(ShortProjectPath("/home/sander/code/artisan", "artisan", "backslash")).toBe(
			"~\\code",
		);
	});
});
