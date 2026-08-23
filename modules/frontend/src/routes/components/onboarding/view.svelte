<script lang="ts" effect>
	import ArrowRight from "@tabler/icons-svelte/icons/arrow-right";
	import CircleCheck from "@tabler/icons-svelte/icons/circle-check";
	import Download from "@tabler/icons-svelte/icons/download";
	import Loader2 from "@tabler/icons-svelte/icons/loader-2";
	import Login from "@tabler/icons-svelte/icons/login";
	import TestPipe from "@tabler/icons-svelte/icons/test-pipe";
	import { Effect, Stream } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import { Button } from "$lib/components/ui/button";
	import {
		Tooltip,
		TooltipContent,
		TooltipProvider,
		TooltipTrigger,
	} from "$lib/components/ui/tooltip";
	import { changed_files_style_config } from "$lib/conversation-style-config";
	import { EngineMarkClass, EngineMarkFor } from "$lib/engine/presentation";
	import {
		EngineUsageController,
		type EngineUsageState,
	} from "$lib/identity/engine-usage-controller";
	import {
		EngineInstallationsController,
		type EngineInstallationsState,
	} from "$lib/settings/engine-installations-controller";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import ShaderGlassSurface from "../shader-glass-surface.svelte";
	import SetupLabel from "./setup-label.svelte";
	import {
		ProjectManagedHarnessSetup,
		type HarnessSetupState,
	} from "./setup-state";

	const hermes_setup_url =
		"https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models";
	const harnesses = [
		{
			id: "codex",
			title: "Codex",
			description: "OpenAI's terminal coding agent for GPT and Codex models.",
			button_color: "#000000",
			experimental: false,
			phase: 0,
			x: -0.4,
			y: 0.1,
		},
		{
			id: "claude",
			title: "Claude Code",
			description: "Anthropic's terminal coding agent for Claude models.",
			button_color: "#D97757",
			experimental: false,
			phase: 3.7,
			x: 0.25,
			y: -0.2,
		},
		{
			id: "cursor",
			title: "Cursor",
			description: "Cursor's CLI agent, using the models enabled on your account.",
			button_color: "#1B1913",
			experimental: true,
			phase: 7.9,
			x: 0.55,
			y: 0.3,
		},
		{
			id: "grok",
			title: "Grok",
			description: "xAI's Grok Build coding agent and model catalog.",
			button_color: "#000000",
			experimental: true,
			phase: 11.4,
			x: -0.15,
			y: 0.5,
		},
		{
			id: "opencode2",
			title: "OpenCode",
			description: "Open-source terminal agent with built-in multi-provider support.",
			button_color: "#211E1E",
			experimental: true,
			phase: 20.1,
			x: -0.5,
			y: -0.25,
		},
		{
			id: "hermes",
			title: "Hermes",
			description: "Nous Research's terminal agent with tools, subagents, and provider profiles.",
			button_color: "#0000F2",
			external_auth: true,
			experimental: true,
			phase: 15.8,
			x: 0.4,
			y: -0.45,
		},
	] as const;

	type Harness = (typeof harnesses)[number];

	const installations_controller = yield* EngineInstallationsController;
	const usage_controller = yield* EngineUsageController;
	const defaults_controller = yield* SessionDefaultsController;
	const navigation = yield* RouteNavigation;
	let completion_error = $state<string | undefined>();
	let completion_saving = $state(false);
	let installations_state = $state.raw<EngineInstallationsState>(
		yield* installations_controller.Current,
	);
	let usage_state = $state.raw<EngineUsageState>(yield* usage_controller.Current);

	yield* installations_controller.Changes.pipe(
		Stream.runForEach((next) =>
			Effect.gen(function* () {
				const prior = installations_state;
				installations_state = next;
				const setup_completed = harnesses.some((harness) => {
					const before = prior.reports[harness.id];
					const after = next.reports[harness.id];
					return (
						(before?.activity === "installing" ||
							before?.activity === "authenticating") &&
						after?.activity === "idle" &&
						after.managed
					);
				});
				if (setup_completed) yield* defaults_controller.Refresh.pipe(Effect.ignore);
			}),
		),
		Effect.forkScoped,
	);
	yield* usage_controller.Changes.pipe(
		Stream.runForEach((next) =>
			Effect.sync(() => {
				usage_state = next;
			}),
		),
		Effect.forkScoped,
	);

	const RefreshSetup = Effect.all(
		[
			installations_controller.Refresh().pipe(Effect.ignore),
			...harnesses.map((harness) =>
				usage_controller.Load(harness.id, { force: true }).pipe(Effect.ignore),
			),
		],
		{ concurrency: "unbounded", discard: true },
	);
	yield* RefreshSetup.pipe(Effect.forkScoped);

	const SetupFor = (harness: Harness): HarnessSetupState => {
		const usage_entry = usage_state.entries.get(harness.id);
		return ProjectManagedHarnessSetup({
			available: installations_state.available,
			error: installations_state.errors[harness.id],
			external_auth: "external_auth" in harness && harness.external_auth,
			pending: installations_state.pending_engine_ids.has(harness.id),
			report: installations_state.reports[harness.id],
			usage: usage_entry?.report,
		});
	};

	const OpenExternal = (url: string) =>
		RunBrowserDom(() => {
			window.open(url, "_blank", "noopener,noreferrer");
		});

	const RunSetup = (harness: Harness) =>
		Effect.gen(function* () {
			const setup = SetupFor(harness);
			switch (setup.action) {
				case "install":
					yield* installations_controller.Install(harness.id);
					return;
				case "authenticate": {
					const next = yield* installations_controller.Authenticate(harness.id);
					const authorization = next.reports[harness.id]?.authorization;
					if (authorization !== undefined) yield* OpenExternal(authorization.url);
					return;
				}
				case "open_authorization":
					if (setup.authorization_url !== undefined)
						yield* OpenExternal(setup.authorization_url);
					return;
				case "open_external_setup":
					yield* OpenExternal(hermes_setup_url);
					return;
				case "none":
					return;
			}
		}).pipe(Effect.catch(() => Effect.void));

	const SetupIsActionable = (setup: HarnessSetupState): boolean =>
		setup.action !== "none";

	const SetupIcon = (setup: HarnessSetupState): "download" | "login" | "ready" => {
		if (setup.ready) return "ready";
		if (setup.action === "install") return "download";
		return "login";
	};

	const CompleteOnboarding = Effect.gen(function* () {
		if (completion_saving) return;
		completion_error = undefined;
		completion_saving = true;
		const completed = yield* defaults_controller
			.SetOnboardingCompleted(true)
			.pipe(Effect.result);
		completion_saving = false;
		if (completed._tag === "Failure") {
			completion_error = "Onboarding could not be saved. Try again.";
			return;
		}
		yield* navigation.Navigate("/");
	});
</script>

<svelte:window onfocus={yield* RefreshSetup} />

	<main class="fixed inset-0 z-40 grid place-items-center overflow-y-auto bg-background p-6 text-foreground">
		<section
			class="flex w-fit max-w-none flex-col gap-8 overflow-hidden rounded-2xl bg-linear-to-t from-(--onboarding-card-from) to-(--onboarding-card-to) p-8"
			class:card={$changed_files_style_config.use_card}
			style:--onboarding-card-from={`var(--${$changed_files_style_config.from})`}
			style:--onboarding-card-to={`var(--${$changed_files_style_config.to})`}
		>
			<header>
				<h1 class="text-lg font-semibold">Set up your harnesses</h1>
			</header>

			<TooltipProvider delayDuration={150}>
				<div class="grid grid-cols-3 gap-3">
					{#each harnesses as harness (harness.id)}
						{@const mark = EngineMarkFor(harness.id)}
						{@const HarnessIcon = mark.icon}
						{@const setup = SetupFor(harness)}
						{@const setup_icon = SetupIcon(setup)}
						<ShaderGlassSurface
							use_card={false}
							ray_offset_x={harness.x}
							ray_offset_y={harness.y}
							ray_time_offset={harness.phase}
							class="card aspect-3/2 h-52 shrink-0 rounded-xl"
							style={`background-color: color-mix(in oklab, ${harness.button_color} 50%, var(--background));`}
						>
							<article class="flex size-full flex-col justify-between p-4">
								<div class="flex flex-col gap-3">
									<div class="flex min-w-0 items-center gap-1.5">
										<HarnessIcon class={EngineMarkClass(mark, "size-4")} />
										<h2 class="truncate text-sm font-medium">{harness.title}</h2>
										{#if setup.ready}
											<span
												class="t-success-check shrink-0"
												style={`color: color-mix(in oklab, var(--foreground) 65%, oklch(from ${harness.button_color} 0.62 c h));`}
												data-state="in"
												aria-label="Installed and signed in"
											>
												<svg class="size-3.5" viewBox="0 0 16 16" fill="none" aria-hidden="true">
													<path
														d="M4 8.5 7 11.5 13 5.5"
														stroke="currentColor"
														stroke-linecap="round"
														stroke-linejoin="round"
														stroke-width="1.75"
													/>
												</svg>
											</span>
										{:else if harness.experimental}
											<Tooltip>
												<TooltipTrigger>
													{#snippet child({ props })}
														<button
															{...props}
															type="button"
															aria-label={`${harness.title} experimental support`}
															class="-m-1 inline-flex size-6 shrink-0 cursor-help items-center justify-center self-center rounded-sm p-1 outline-none focus-visible:ring-2 focus-visible:ring-foreground/50"
															style={`color: color-mix(in oklab, var(--foreground) 65%, oklch(from ${harness.button_color} 0.62 c h));`}
														>
															<TestPipe class="size-3.5" aria-hidden="true" />
														</button>
													{/snippet}
												</TooltipTrigger>
												<TooltipContent
													arrow={false}
													side="top"
													sideOffset={8}
													class="z-[80] block max-w-64 rounded-2xl bg-transparent! p-0! text-foreground! shadow-none! ring-0!"
												>
													<ShaderGlassSurface strength="strong" class="w-full rounded-2xl" use_rays={false}>
														<span class="block px-3 py-2 text-pretty text-xs text-muted-foreground">
															This harness has <span style={`color: color-mix(in oklab, var(--foreground) 65%, oklch(from ${harness.button_color} 0.62 c h));`}>experimental</span>
															support and is not fully tested.
														</span>
													</ShaderGlassSurface>
												</TooltipContent>
											</Tooltip>
										{/if}
									</div>
									<p
										class="max-w-64 text-sm leading-5"
										style={`color: color-mix(in oklab, var(--foreground) 72%, ${harness.button_color});`}
									>
										{harness.description}
									</p>
									{#if setup.failure !== undefined}
										<p class="max-w-64 text-xs leading-4 text-destructive">
											{setup.failure}
										</p>
									{/if}
								</div>

								<Button
									class={`card-plastic w-full rounded-[10px] text-sm font-normal text-background transition-opacity duration-(--duration-fast) ease-(--ease-in-out) ${SetupIsActionable(setup) ? "hover:opacity-90" : "cursor-default"}`}
									style={`background-color: color-mix(in oklab, var(--foreground) 65%, oklch(from ${harness.button_color} 0.62 c h));`}
									aria-busy={setup.busy}
									aria-disabled={!SetupIsActionable(setup)}
									onclick={yield* RunSetup(harness)}
								>
									<span
										class="t-icon-swap size-4 text-background/85"
										data-state={setup.busy ? "b" : "a"}
										aria-hidden="true"
									>
										<span class="t-icon" data-icon="a">
											{#if setup_icon === "login"}
												<Login />
											{:else if setup_icon === "ready"}
												<CircleCheck />
											{:else}
												<Download />
											{/if}
										</span>
										<span class="t-icon" data-icon="b">
											<Loader2 class="animate-spin" />
										</span>
									</span>
									<SetupLabel label={setup.label} email={setup.email} />
								</Button>
							</article>
						</ShaderGlassSurface>
					{/each}
				</div>
			</TooltipProvider>

			<footer class="flex justify-end">
				<div class="flex flex-col items-end gap-2">
					{#if completion_error !== undefined}
						<p class="text-xs text-destructive" role="status">{completion_error}</p>
					{/if}
					<Button
						disabled={completion_saving}
						class="card-plastic rounded-[10px] bg-foreground text-sm font-normal text-background hover:bg-foreground/90"
						onclick={yield* CompleteOnboarding}
					>
						{completion_saving ? "Saving…" : "Continue"}
						<ArrowRight class="text-background/85" aria-hidden="true" />
					</Button>
				</div>
			</footer>
		</section>
	</main>
