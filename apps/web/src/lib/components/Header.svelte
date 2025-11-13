<script lang="ts">
  import { Button } from "$lib/components/ui/button/index.js";
  import { SiGithub } from "@icons-pack/svelte-simple-icons";
  import { toggleMode } from "mode-watcher";
  import { Moon, Sun } from "@lucide/svelte";

  let { isScrolled = false } = $props<{ isScrolled?: boolean }>();
</script>

<header
  class="sticky top-0 w-full border-b bg-background transition-all duration-300 ease-out {isScrolled
    ? ''
    : 'border-transparent'}"
>
  <div
    class="mx-auto flex h-(--header-height) max-w-(--ds-page-width-with-margin) items-center justify-between px-6"
  >
    <a href="/" class="text-xl font-medium">BMXtract</a>
    <div class="flex gap-1">
      <Button onclick={toggleMode} variant="ghost" size="icon">
        <Sun
          class="h-full w-full scale-100 rotate-0 transition-all! dark:scale-0 dark:-rotate-90"
        />
        <Moon
          class="absolute h-full w-full scale-0 rotate-90 transition-all! dark:scale-100 dark:rotate-0"
        />
        <span class="sr-only">Toggle theme</span>
      </Button>
      <Button
        href="https://github.com/holybaechu/bmxtract"
        target="_blank"
        variant="ghost"
        size="icon"><SiGithub /></Button
      >
    </div>
  </div>
</header>
