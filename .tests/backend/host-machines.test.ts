import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";

import {
	build_machines_snapshot,
	HostMachinesService,
	make_host_machines_layer,
	parse_wsl_distributions,
} from "../../modules/backend/src/runtime/host-machines";
import { type HostIdentityCommandRunnerShape } from "../../modules/backend/src/runtime/host-identity";

function fake_runner(
	run: (command: string, args: ReadonlyArray<string>) => Effect.Effect<string, unknown>,
): HostIdentityCommandRunnerShape {
	return { Run: run };
}

/** Rebuilds the interleaved-NUL shape a UTF-8 capture of UTF-16LE output has. */
function utf16_like(text: string): string {
	return [...text].map((character) => `${character}\u0000`).join("");
}

describe("host machines service", () => {
	it("parses distribution names from a UTF-16 flavoured wsl.exe listing", () => {
		expect(parse_wsl_distributions(`\uFEFF${utf16_like("Ubuntu\r\nartisan\r\n")}`)).toEqual([
			"Ubuntu",
			"artisan",
		]);
		expect(parse_wsl_distributions("Ubuntu\nDebian\n")).toEqual(["Ubuntu", "Debian"]);
		expect(parse_wsl_distributions("")).toEqual([]);
		expect(parse_wsl_distributions("\r\n \r\n")).toEqual([]);
	});

	it("filters container-tooling utility distributions from the listing", () => {
		expect(
			parse_wsl_distributions(
				"Ubuntu\ndocker-desktop\ndocker-desktop-data\nrancher-desktop\n",
			),
		).toEqual(["Ubuntu"]);
	});

	it("labels the Forge host machine 'This computer' and lists distributions after it", () => {
		const snapshot = build_machines_snapshot("win32", "DESKTOP-1", undefined, [
			"Ubuntu",
			"artisan",
		]);

		expect(snapshot.machines).toEqual([
			{ detail: "DESKTOP-1", id: "local", kind: "local", label: "This computer" },
			{ detail: "Ubuntu", id: "wsl:Ubuntu", kind: "wsl", label: "This computer on WSL2" },
			{ detail: "artisan", id: "wsl:artisan", kind: "wsl", label: "This computer on WSL2" },
		]);
	});

	it("labels a WSL-hosted Forge as running on WSL2", () => {
		const snapshot = build_machines_snapshot("linux", "wsl-host", "artisan", []);

		expect(snapshot.machines).toEqual([
			{ detail: "artisan", id: "local", kind: "local", label: "This computer on WSL2" },
		]);
	});

	it("returns only the local machine on hosts without WSL surfaces", () => {
		expect(build_machines_snapshot("darwin", "macbook", undefined, []).machines).toEqual([
			{ detail: "macbook", id: "local", kind: "local", label: "This computer" },
		]);
		expect(build_machines_snapshot("linux", "server", undefined, []).machines).toEqual([
			{ detail: "server", id: "local", kind: "local", label: "This computer" },
		]);
	});

	it.effect("never fails the query when distribution enumeration fails", () =>
		Effect.gen(function* () {
			const host_machines = yield* HostMachinesService;
			const snapshot = yield* host_machines.List;

			expect(snapshot.machines.length).toBeGreaterThan(0);
			expect(snapshot.machines[0]?.kind).toBe("local");
			expect(snapshot.machines[0]?.label.length).toBeGreaterThan(0);
		}).pipe(
			Effect.provide(
				make_host_machines_layer(fake_runner(() => Effect.fail(new Error("boom")))),
			),
		),
	);

	it.effect("enumerates WSL distributions through wsl.exe on Windows hosts", () => {
		const calls: Array<ReadonlyArray<string>> = [];
		const runner = fake_runner((command, args) => {
			calls.push([command, ...args]);
			return Effect.succeed("Ubuntu\ndocker-desktop\n");
		});

		return Effect.gen(function* () {
			const host_machines = yield* HostMachinesService;
			const snapshot = yield* host_machines.List;

			expect(snapshot.machines[0]?.kind).toBe("local");
			if (process.platform === "win32") {
				expect(calls).toEqual([["wsl.exe", "-l", "-q"]]);
				expect(snapshot.machines).toContainEqual({
					detail: "Ubuntu",
					id: "wsl:Ubuntu",
					kind: "wsl",
					label: "This computer on WSL2",
				});
				expect(snapshot.machines.map((machine) => machine.id)).not.toContain(
					"wsl:docker-desktop",
				);
			} else {
				expect(calls).toEqual([]);
			}
		}).pipe(Effect.provide(make_host_machines_layer(runner)));
	});
});
