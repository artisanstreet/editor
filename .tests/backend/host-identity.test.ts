import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";

import {
	build_display_name_command,
	HostIdentityService,
	make_host_identity_layer,
	parse_display_name,
	type HostIdentityCommandRunnerShape,
} from "../../modules/backend/src/runtime/host-identity";

function fake_runner(
	run: (command: string, args: ReadonlyArray<string>) => Effect.Effect<string, unknown>,
): HostIdentityCommandRunnerShape {
	return { Run: run };
}

describe("host identity service", () => {
	it.effect("resolves the display name on the win32 path", () => {
		let calls = 0;
		const runner = fake_runner((command, args) => {
			calls += 1;
			expect(command).toBe("powershell.exe");
			expect(args).toContain("-Command");
			return Effect.succeed("  Jane Doe \r\n");
		});

		return Effect.gen(function* () {
			const host_identity = yield* HostIdentityService;
			const snapshot = yield* host_identity.Get;

			expect(snapshot.display_name).toBe("Jane Doe");
			expect(calls).toBe(1);
		}).pipe(Effect.provide(make_host_identity_layer(runner)));
	});

	it.effect("never fails the query when the platform command fails", () =>
		Effect.gen(function* () {
			const host_identity = yield* HostIdentityService;
			const snapshot = yield* host_identity.Get;

			expect(snapshot.display_name).toBeUndefined();
			expect(snapshot.hostname.length).toBeGreaterThan(0);
		}).pipe(
			Effect.provide(
				make_host_identity_layer(fake_runner(() => Effect.fail(new Error("boom")))),
			),
		),
	);

	it.effect("always reports a nonempty hostname and a known platform literal", () =>
		Effect.gen(function* () {
			const host_identity = yield* HostIdentityService;
			const snapshot = yield* host_identity.Get;

			expect(snapshot.hostname.length).toBeGreaterThan(0);
			expect(["win32", "darwin", "linux", "unknown"]).toContain(snapshot.platform);
		}).pipe(
			Effect.provide(make_host_identity_layer(fake_runner(() => Effect.succeed("Someone")))),
		),
	);

	it.effect("caches the resolved snapshot for the process lifetime", () => {
		let calls = 0;
		const runner = fake_runner(() => {
			calls += 1;
			return Effect.succeed("Cached Name");
		});

		return Effect.gen(function* () {
			const host_identity = yield* HostIdentityService;
			const first = yield* host_identity.Get;
			const second = yield* host_identity.Get;

			expect(first).toEqual(second);
			expect(calls).toBe(1);
		}).pipe(Effect.provide(make_host_identity_layer(runner)));
	});

	it("skips the Windows lookup for a username with unsafe characters", () => {
		expect(build_display_name_command("win32", "bad;name")).toBeUndefined();
		expect(build_display_name_command("win32", undefined)).toBeUndefined();
		expect(build_display_name_command("win32", "sander.smith")).toMatchObject({
			command: "powershell.exe",
		});
	});

	it("builds the darwin and linux lookup commands", () => {
		expect(build_display_name_command("darwin", "sander")).toEqual({
			args: ["-F"],
			command: "id",
		});
		expect(build_display_name_command("linux", "sander")).toEqual({
			args: ["passwd", "sander"],
			command: "getent",
		});
		expect(build_display_name_command("linux", undefined)).toBeUndefined();
		expect(build_display_name_command("unknown", "sander")).toBeUndefined();
	});

	it("parses the GECOS field from a Linux getent row", () => {
		expect(
			parse_display_name("linux", "sander:x:1000:1000:Sander S,,,:/home/sander:/bin/bash"),
		).toBe("Sander S");
		expect(parse_display_name("darwin", "  Sander S  \n")).toBe("Sander S");
		expect(parse_display_name("win32", "")).toBeUndefined();
		expect(parse_display_name("win32", "   \n")).toBeUndefined();
	});
});
