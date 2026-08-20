import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";

import {
	build_wsl_handoff_command,
	HostMachineBrokerService,
	is_loopback_endpoint,
	make_host_machine_broker_layer,
	parse_handoff_output,
} from "../../modules/backend/src/runtime/host-machine-broker";
import { type HostIdentityCommandRunnerShape } from "../../modules/backend/src/runtime/host-identity";

function fake_runner(
	run: (command: string, args: ReadonlyArray<string>) => Effect.Effect<string, unknown>,
): HostIdentityCommandRunnerShape {
	return { Run: run };
}

const handoff_line = JSON.stringify({
	endpoint: "http://127.0.0.1:35785/",
	owned_instance_id: "instance_1",
	pair_code: "ABCD-EFGH",
});

describe("host machine broker", () => {
	it("builds the wsl.exe argv with the default and overridden ae command", () => {
		expect(build_wsl_handoff_command("artisan", undefined)).toEqual({
			args: ["-d", "artisan", "--", "ae", "open", "--handoff"],
			command: "wsl.exe",
		});
		expect(build_wsl_handoff_command("artisan", "sh /root/artisan/ae.sh")).toEqual({
			args: ["-d", "artisan", "--", "sh", "/root/artisan/ae.sh", "open", "--handoff"],
			command: "wsl.exe",
		});
	});

	it("parses the handoff line out of surrounding runtime chatter", () => {
		expect(parse_handoff_output(handoff_line)).toEqual({
			endpoint: "http://127.0.0.1:35785/",
			pair_code: "ABCD-EFGH",
		});
		expect(parse_handoff_output(`starting forge...\n${handoff_line}\ndone\n`)).toEqual({
			endpoint: "http://127.0.0.1:35785/",
			pair_code: "ABCD-EFGH",
		});
		expect(parse_handoff_output("no json here\n{not json}\n")).toBeUndefined();
		expect(parse_handoff_output('{"endpoint":"","pair_code":"X"}')).toBeUndefined();
		expect(parse_handoff_output('{"endpoint":"http://127.0.0.1:1/"}')).toBeUndefined();
		expect(parse_handoff_output("")).toBeUndefined();
	});

	it("accepts only loopback http endpoints", () => {
		expect(is_loopback_endpoint("http://127.0.0.1:35785/")).toBe(true);
		expect(is_loopback_endpoint("http://localhost:8080/")).toBe(true);
		expect(is_loopback_endpoint("http://192.168.1.20:8080/")).toBe(false);
		expect(is_loopback_endpoint("https://127.0.0.1:8080/")).toBe(false);
		expect(is_loopback_endpoint("not a url")).toBe(false);
	});

	it.effect("refuses machine ids that are not WSL distributions", () =>
		Effect.gen(function* () {
			const broker = yield* HostMachineBrokerService;

			const local = yield* broker.Connect("local");
			expect(local).toMatchObject({ reason: "unknown_machine", status: "failed" });

			const injected = yield* broker.Connect("wsl:bad name; rm -rf");
			expect(injected).toMatchObject({ reason: "unknown_machine", status: "failed" });
		}).pipe(
			Effect.provide(
				make_host_machine_broker_layer(
					fake_runner(() => Effect.die("runner must not be called")),
				),
			),
		),
	);

	it.effect("connects through a faked wsl.exe handoff on Windows hosts", () => {
		const calls: Array<ReadonlyArray<string>> = [];
		const runner = fake_runner((command, args) => {
			calls.push([command, ...args]);
			return Effect.succeed(`booting\n${handoff_line}\n`);
		});

		return Effect.gen(function* () {
			const broker = yield* HostMachineBrokerService;
			const outcome = yield* broker.Connect("wsl:artisan");

			if (process.platform === "win32") {
				expect(outcome).toEqual({
					endpoint: "http://127.0.0.1:35785/",
					pair_code: "ABCD-EFGH",
					status: "connected",
				});
				expect(calls[0]?.[0]).toBe("wsl.exe");
				expect(calls[0]).toContain("artisan");
			} else {
				expect(outcome).toMatchObject({ reason: "start_failed", status: "failed" });
				expect(calls).toEqual([]);
			}
		}).pipe(Effect.provide(make_host_machine_broker_layer(runner)));
	});

	it.effect("maps a failed launch and a missing handoff to terminal failures", () =>
		Effect.gen(function* () {
			if (process.platform !== "win32") return;

			const failing = yield* HostMachineBrokerService.pipe(
				Effect.provide(
					make_host_machine_broker_layer(
						fake_runner(() => Effect.fail(new Error("boom"))),
					),
				),
			);
			expect(yield* failing.Connect("wsl:artisan")).toMatchObject({
				reason: "start_failed",
				status: "failed",
			});

			const silent = yield* HostMachineBrokerService.pipe(
				Effect.provide(
					make_host_machine_broker_layer(fake_runner(() => Effect.succeed("no handoff"))),
				),
			);
			expect(yield* silent.Connect("wsl:artisan")).toMatchObject({
				reason: "start_failed",
				status: "failed",
			});

			const hostile = yield* HostMachineBrokerService.pipe(
				Effect.provide(
					make_host_machine_broker_layer(
						fake_runner(() =>
							Effect.succeed(
								JSON.stringify({
									endpoint: "http://192.168.1.5:9999/",
									pair_code: "ABCD-EFGH",
								}),
							),
						),
					),
				),
			);
			expect(yield* hostile.Connect("wsl:artisan")).toMatchObject({
				reason: "start_failed",
				status: "failed",
			});
		}),
	);
});
