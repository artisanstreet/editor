<script lang="ts" effect>
	import { goto } from "$app/navigation";
	import { page } from "$app/state";
	import { Effect, Option } from "effect";
	import type { DesktopIdentity } from "@artisan/transport/client";
	import {
		IconHome as Home,
		IconMessagePlus as MessagePlus,
		IconSettings as Settings,
		IconSparkles as Sparkles,
	} from "@tabler/icons-svelte";

	import { Button } from "$lib/components/ui/button";
	import {
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuLabel,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
	} from "$lib/components/ui/dropdown-menu";
	import { LiveWorkspaceStore, type LiveWorkspaceSnapshot } from "$lib/live-workspace/store";

	let {
		live_snapshot,
		identity,
		on_create_thread,
	}: {
		live_snapshot: LiveWorkspaceSnapshot;
		identity: DesktopIdentity;
		on_create_thread: (title: string) => Effect.Effect<void>;
	} = $props();
	const live_workspace = yield* LiveWorkspaceStore;

	const setting_sections = [
		["general", "General"],
		["codex", "Codex"],
		["guidance", "Guidance"],
		["model-behaviour", "Model behaviour"],
		["retention", "Retention"],
		["appearance", "Appearance"],
	] as const;
	const initials = $derived(
		identity.display_name
			.split(/\s+/u)
			.filter(Boolean)
			.slice(0, 2)
			.map((part) => part[0]?.toUpperCase() ?? "")
			.join("") || "AE",
	);
	const avatar_hue = $derived(
		[...identity.avatar_seed].reduce(
			(hash, character) => (hash * 31 + (character.codePointAt(0) ?? 0)) % 360,
			0,
		),
	);
	const settings_active = $derived(page.url.pathname === "/settings");
	const current_thread_id = $derived(Option.getOrUndefined(live_snapshot.selected_thread_id));

	const CreateThread = Effect.gen(function* () {
		yield* on_create_thread("New chat");
		const latest_snapshot = yield* live_workspace.Snapshot;
		const created_thread_id = Option.getOrUndefined(latest_snapshot.selected_thread_id);
		if (created_thread_id !== undefined)
			yield* Effect.tryPromise(() => goto(`/thread/${created_thread_id}`));
	});
</script>

<aside class="app-sidebar" aria-label="Primary navigation">
	<header class="brand-row">
		<a class="brand-link" href="/" aria-label="Artisan Editor home">
			<img class="brand-mark" src="/barekey-logo.png" alt="" />
			<span>Artisan Editor</span>
		</a>
	</header>

	<nav class="top-actions" aria-label="Workspace">
		<Button href="/" variant={page.url.pathname === "/" ? "secondary" : "ghost"} class="w-full justify-start gap-2">
			<Home size={17} stroke={1.7} aria-hidden="true" /> Home
		</Button>
		<Button aria-label="New chat" variant="outline" class="w-full justify-start gap-2" onclick={yield* CreateThread}>
			<MessagePlus size={17} stroke={1.7} aria-hidden="true" /> New thread
		</Button>
	</nav>

	<section class="sidebar-list" aria-label={settings_active ? "Settings sections" : "Recent threads"}>
		<div class="sidebar-heading">{settings_active ? "Settings" : "Recent threads"}</div>
		{#if settings_active}
			<nav class="section-links" aria-label="Settings sections">
				{#each setting_sections as [section_id, label]}
					<a href={`/settings#${section_id}`}>{label}</a>
				{/each}
			</nav>
		{:else if live_snapshot.threads.length === 0}
			<p class="empty-state">{live_snapshot.phase === "error" ? "Desktop session unavailable." : "Your recent threads will appear here."}</p>
		{:else}
			<nav class="thread-links" aria-label="Recent threads">
				{#each live_snapshot.threads.slice(0, 12) as thread}
					<a class:active={current_thread_id === thread.thread_id} href={`/thread/${thread.thread_id}`}>
						<span class="truncate">{thread.title}</span>
						<span class="thread-meta">{thread.live_status}</span>
					</a>
				{/each}
			</nav>
		{/if}
	</section>

	<footer class="identity-row">
		<div class="avatar" style={`--avatar-hue:${avatar_hue}`} aria-hidden="true">
			{#if identity.avatar_data_url}<img src={identity.avatar_data_url} alt="" />{:else}{initials}{/if}
		</div>
		<div class="identity-copy"><strong>{identity.display_name}</strong><span>{identity.machine_name}</span></div>
		<DropdownMenu>
			<DropdownMenuTrigger>
				{#snippet child({ props })}
					<Button {...props} variant="ghost" size="icon-sm" aria-label="Open account menu"><Sparkles size={16} /></Button>
				{/snippet}
			</DropdownMenuTrigger>
			<DropdownMenuContent align="end">
				<DropdownMenuLabel>{identity.display_name}</DropdownMenuLabel>
				<DropdownMenuSeparator />
				<DropdownMenuItem href="/settings"><Settings size={15} /> Settings</DropdownMenuItem>
			</DropdownMenuContent>
		</DropdownMenu>
	</footer>
</aside>

<style>
	.app-sidebar { display: flex; min-height: 0; flex-direction: column; border: 1px solid var(--line); border-radius: 1.5rem; background: var(--pane); overflow: hidden; }
	.brand-row { padding: 0.75rem; border-bottom: 1px solid var(--line); }
	.brand-link { display: flex; align-items: center; gap: 0.625rem; color: var(--text-primary); font-size: 0.9375rem; font-weight: 700; letter-spacing: -0.035em; text-decoration: none; }
	.brand-mark { width: 2rem; height: 2rem; border-radius: 0.625rem; }
	.top-actions { display: grid; gap: 0.25rem; padding: 0.625rem; }
	.sidebar-list { display: flex; min-height: 0; flex: 1; flex-direction: column; padding: 0.25rem 0.625rem 0.625rem; overflow: hidden; }
	.sidebar-heading { padding: 0.5rem; color: var(--text-muted); font-size: 0.6875rem; font-weight: 700; letter-spacing: 0.09em; text-transform: uppercase; }
	.thread-links, .section-links { display: grid; gap: 0.125rem; overflow-y: auto; overscroll-behavior: contain; }
	.thread-links a, .section-links a { display: grid; gap: 0.125rem; border-radius: 0.75rem; padding: 0.5rem 0.625rem; color: var(--text-muted); font-size: 0.8125rem; text-decoration: none; transition: background 150ms ease, color 150ms ease; }
	.thread-links a:hover, .thread-links a.active, .section-links a:hover { background: var(--pane-inset); color: var(--text-primary); }
	.thread-meta { color: var(--text-muted); font-size: 0.6875rem; }
	.empty-state { margin: 0.5rem; color: var(--text-muted); font-size: 0.75rem; line-height: 1.5; }
	.identity-row { display: flex; align-items: center; gap: 0.625rem; min-height: 4rem; padding: 0.625rem; border-top: 1px solid var(--line); background: var(--pane-inset); }
	.avatar { display: grid; width: 2rem; height: 2rem; flex: 0 0 auto; place-items: center; border: 1px solid var(--line-strong); border-radius: 999px; background: linear-gradient(135deg, hsl(var(--avatar-hue) 72% 58%), hsl(calc(var(--avatar-hue) + 54) 68% 42%)); color: white; font-size: 0.6875rem; font-weight: 700; }
	.avatar img { width: 100%; height: 100%; border-radius: inherit; object-fit: cover; }
	.identity-copy { display: grid; min-width: 0; flex: 1; font-size: 0.75rem; }
	.identity-copy strong, .identity-copy span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.identity-copy span { color: var(--text-muted); font-size: 0.6875rem; }
	.truncate { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	@media (max-width: 760px) { .app-sidebar { display: none; } }
</style>
