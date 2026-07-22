<script lang="ts">
	import barekey_logo from "$lib/assets/barekey/logo-40.png";
	import * as Sidebar from "$lib/components/ui/sidebar";
</script>

<div class="t-sidebar-flyout flex flex-1 flex-col">
	<Sidebar.Header class="pl-6 pr-14 lg:pl-2">
		<div
			class="t-sidebar-flyout-inline inline-block min-w-0 max-w-[var(--sidebar-flyout-inline-width,none)] overflow-visible whitespace-nowrap"
		>
			<a href="/" class="t-sidebar-child -ml-1 flex flex-row items-center gap-2 lg:ml-0">
				<img src={barekey_logo} alt="" class="size-5 shrink-0 invert dark:invert-0" />
				<span class="font-logo">Artisan Editor</span>
			</a>
		</div>
	</Sidebar.Header>

	<Sidebar.Content
		class="docs-sidebar-nav-surface docs-scroll-fade relative overflow-x-hidden px-2 pb-3"
	/>
</div>
