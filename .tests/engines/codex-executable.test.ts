import { describe, expect, it } from "vitest";

import { make_codex_process_environment, resolve_codex_executable } from "@artisan/engines";

describe("Codex executable discovery", () => {
	it("honors an explicit Artisan executable before every platform discovery source", () => {
		expect(
			resolve_codex_executable({
				environment: {
					ARTISAN_CODEX_EXECUTABLE: "C:\\tools\\codex.exe",
					LOCALAPPDATA: "C:\\Users\\artisan\\AppData\\Local",
				},
				Exists: () => false,
				platform: "win32",
			}),
		).toBe("C:\\tools\\codex.exe");
	});

	it("prefers the directly executable OpenAI cache over a restricted Winget package", () => {
		const local_app_data = "C:\\Users\\artisan\\AppData\\Local";
		const winget = `${local_app_data}\\Microsoft\\WinGet\\Packages\\OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe\\codex-x86_64-pc-windows-msvc.exe`;
		const selected = `${local_app_data}\\OpenAI\\Codex\\bin\\0.142.5\\codex.exe`;

		expect(
			resolve_codex_executable({
				environment: { LOCALAPPDATA: local_app_data, PATH: "C:\\Tools" },
				Exists: (path) => path === winget || path === selected,
				platform: "win32",
				ReadDirectory: () => ["0.141.0", "0.142.5"],
			}),
		).toBe(selected);
	});

	it("uses the newest deterministic OpenAI local installation before ordinary PATH", () => {
		const local_app_data = "C:\\Users\\artisan\\AppData\\Local";
		const selected = `${local_app_data}\\OpenAI\\Codex\\bin\\0.142.5\\codex.exe`;

		expect(
			resolve_codex_executable({
				environment: { LOCALAPPDATA: local_app_data, PATH: "C:\\Tools" },
				Exists: (path) => path === selected || path === "C:\\Tools\\codex.exe",
				platform: "win32",
				ReadDirectory: () => ["0.99.0", "0.142.5"],
			}),
		).toBe(selected);
	});

	it("does not treat the inaccessible WindowsApps alias as a usable PATH installation", () => {
		const local_app_data = "C:\\Users\\artisan\\AppData\\Local";

		expect(
			resolve_codex_executable({
				environment: {
					LOCALAPPDATA: local_app_data,
					PATH: `${local_app_data}\\Microsoft\\WindowsApps;C:\\Tools`,
				},
				Exists: (path) => path.endsWith("WindowsApps\\codex.exe"),
				platform: "win32",
				ReadDirectory: () => [],
			}),
		).toBe(`${local_app_data}\\OpenAI\\Codex\\bin\\codex.exe`);
	});

	it("keeps an existing Codex home and otherwise derives it from USERPROFILE", () => {
		expect(
			make_codex_process_environment({
				CODEX_HOME: "D:\\codex-home",
				USERPROFILE: "C:\\Users\\artisan",
			}),
		).toMatchObject({ CODEX_HOME: "D:\\codex-home" });
		expect(make_codex_process_environment({ USERPROFILE: "C:\\Users\\artisan" })).toMatchObject(
			{ CODEX_HOME: "C:\\Users\\artisan\\.codex" },
		);
	});

	it("merges inherited variables while preserving explicit spawn input precedence", () => {
		expect(
			make_codex_process_environment(
				{ PATH: "D:\\explicit-bin", USERPROFILE: "D:\\explicit-user" },
				{ ARTISAN_TOKEN: "inherited", PATH: "C:\\inherited-bin" },
				"C:\\runtime-user",
			),
		).toEqual({
			ARTISAN_TOKEN: "inherited",
			CODEX_HOME: "D:\\explicit-user\\.codex",
			PATH: "D:\\explicit-bin",
			USERPROFILE: "D:\\explicit-user",
		});
	});

	it("uses the runtime profile and local app data without ambient Node reads", () => {
		expect(make_codex_process_environment({}, { PATH: "C:\\bin" }, "C:\\runtime-user")).toEqual(
			{
				CODEX_HOME: "C:\\runtime-user\\.codex",
				PATH: "C:\\bin",
			},
		);
		expect(
			resolve_codex_executable({
				environment: {},
				local_app_data: "D:\\Local",
				platform: "win32",
			}),
		).toBe("D:\\Local\\OpenAI\\Codex\\bin\\codex.exe");
	});
});
