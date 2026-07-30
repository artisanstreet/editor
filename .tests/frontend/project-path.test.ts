import { describe, expect, it } from "vitest";

import { ShortProjectPath } from "../../modules/frontend/src/lib/root/project-path";

describe("short project path", () => {
	it("collapses a Windows home directory and drops the repeated project name", () => {
		expect(
			ShortProjectPath("C:\\Users\\sander\\Desktop\\artisan-editor", "artisan-editor"),
		).toBe("~/Desktop");
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
			"~/Desktop/artisan-editor",
		);
	});

	it("normalizes separators without a home directory to collapse", () => {
		expect(ShortProjectPath("D:\\work\\repos\\artisan", "artisan")).toBe("D:/work/repos");
	});

	it("reports nothing when the project sits directly in the home directory", () => {
		expect(ShortProjectPath("/home/sander/artisan", "artisan")).toBeUndefined();
	});

	it("ignores a trailing separator", () => {
		expect(ShortProjectPath("/home/sander/code/artisan/", "artisan")).toBe("~/code");
	});

	it("does not mistake a home-named directory elsewhere for the home root", () => {
		expect(ShortProjectPath("/srv/home/sander/artisan", "artisan")).toBe("/srv/home/sander");
	});
});
