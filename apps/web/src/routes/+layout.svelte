<script lang="ts">
  import "../app.css";
  import favicon from "$lib/assets/favicon.svg";
  import { ModeWatcher, mode } from "mode-watcher";
  import Header from "$lib/components/Header.svelte";
  import { Toaster } from "svelte-sonner";
  import { injectSpeedInsights } from "@vercel/speed-insights/sveltekit";
  import { injectAnalytics } from "@vercel/analytics/sveltekit";

  injectSpeedInsights();
  injectAnalytics();

  let isScrolled = $state(false);
  $effect(() => {
    window.addEventListener("scroll", () => {
      if (window.scrollY > 0) {
        isScrolled = true;
      } else {
        isScrolled = false;
      }
    });
  });

  let { children } = $props();
</script>

<svelte:head>
  <link rel="icon" href={favicon} />
</svelte:head>

<ModeWatcher />
<Toaster theme={mode.current} />
<div class="flex min-h-screen flex-col items-center">
  <Header {isScrolled} />

  <main class="flex w-full max-w-(--ds-page-width-with-margin) flex-1 flex-col px-6">
    {@render children()}
  </main>

  <footer class="flex h-(--footer-height) w-full items-center justify-center">
    <p class="text-sm text-muted-foreground">
      Built by <a href="https://holyb.xyz/" target="_blank" class="underline">holybaechu</a>
    </p>
  </footer>
</div>
