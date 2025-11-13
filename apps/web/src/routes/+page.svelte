<script lang="ts">
  import { Button, buttonVariants } from "$lib/components/ui/button/index.ts";
  import * as Dialog from "$lib/components/ui/dialog/index.ts";
  import * as Select from "$lib/components/ui/select/index.ts";
  import { Card } from "$lib/components/ui/card/index.ts";
  import {
    PlusIcon,
    UploadIcon,
    CheckIcon,
    TrashIcon,
    DownloadIcon,
    RefreshCwIcon,
  } from "@lucide/svelte";
  import { Checkbox } from "$lib/components/ui/checkbox/index.ts";
  import { onMount } from "svelte";
  import { SvelteMap } from "svelte/reactivity";
  import { toast } from "svelte-sonner";
  import { BmsQueueItem } from "$lib/models/BmsQueueItem";
  import { collectFilesFromDirectory } from "$lib/utils/fileSystem";
  import { MessageType, type Message } from "$lib/types";
  import { BlobWriter, ZipWriter, Uint8ArrayReader } from "@zip.js/zip.js";
  import { Spinner } from "$lib/components/ui/spinner/index.ts";

  let selectedChart = $state<string | undefined>();
  let dropZone: HTMLDivElement;
  let itemToAdd = $state<BmsQueueItem | null>(null);
  let bmsQueue = new SvelteMap<string, BmsQueueItem>();
  let useFloat32 = $state(false);
  let createdUrls: string[] = [];

  const triggerContent = $derived(
    itemToAdd?.files.find((f) => {
      return f.name === selectedChart;
    })?.name ?? `Select a chart`,
  );
  const bmsFiles = $derived(
    itemToAdd?.files.filter((f) => f.name.endsWith(".bms") || f.name.endsWith(".bme")),
  );

  const ondragover = (e: DragEvent) => {
    if (!e.dataTransfer) return;

    // webkitGetAsEntry is not available for dragover events
    const files = [...e.dataTransfer.items].filter((f) => f.kind === "file");
    if (files.length === 0) return;

    e.preventDefault();
    if (dropZone && dropZone.contains(e.target as Node)) {
      e.dataTransfer.dropEffect = "copy";
      return;
    }

    e.dataTransfer.dropEffect = "none";
  };

  const ondrop = async (e: DragEvent) => {
    if (itemToAdd) return;
    if (!e.dataTransfer) return;

    const directories = [...e.dataTransfer.items]
      .map((item) => item.webkitGetAsEntry())
      .filter((entry): entry is FileSystemDirectoryEntry => entry?.isDirectory ?? false);
    if (directories.length === 0) return;

    e.preventDefault();

    // Only allow one directory to be dropped
    const [directory] = directories;

    try {
      const files = await collectFilesFromDirectory(directory);
      if (!files.length) return;

      itemToAdd = new BmsQueueItem(directory.name, files);
    } catch (error) {
      console.error("Failed to read dropped directory", error);
    }
  };

  let dialogOpen = $state(false);
  let renderWorker: Worker | null = null;
  let workerReady = $state(false);

  // Track pending renders
  const pendingRenders = new SvelteMap<
    string,
    {
      resolve: () => void;
      reject: (error: string) => void;
    }
  >();

  onMount(() => {
    const initWorker = async () => {
      const RenderWorker = await import("$lib/renderWorker?worker");
      const worker = new RenderWorker.default();

      worker.onmessage = async (ev: MessageEvent<Message>) => {
        const data = ev.data;

        switch (data.type) {
          case MessageType.INIT:
            toast.success("Renderer initialized.");
            workerReady = true;
            break;

          case MessageType.READ_FILES:
            {
              const { id, paths } = data;
              const item = bmsQueue.get(id);
              if (!item) {
                console.error(`No queue item found for ID: ${id}`);
                return;
              }

              const missingFiles: string[] = [];
              const buffers = await Promise.all(
                paths.map(async (path) => {
                  let file = item.files.find((f) => f.name === path);
                  const fileName = path.split(".").slice(0, -1).join(".");
                  // Try fallback extensions
                  if (!file) file = item.files.find((f) => f.name === fileName + ".wav");
                  if (!file) file = item.files.find((f) => f.name === fileName + ".mp3");
                  if (!file) file = item.files.find((f) => f.name === fileName + ".ogg");
                  if (!file) {
                    console.warn(`File not found: ${path}`);
                    missingFiles.push(path);
                    return new ArrayBuffer(0);
                  }

                  return await file.arrayBuffer();
                }),
              );

              // Store missing files in the item
              if (missingFiles.length > 0) {
                bmsQueue.set(id, { ...item, missingFiles });
                toast.warning(`${missingFiles.length} audio file(s) not found`);
              }

              worker.postMessage(
                {
                  type: MessageType.READ_FILES_RESPONSE,
                  id,
                  buffers,
                },
                buffers,
              );
            }
            break;

          case MessageType.RESULT:
            {
              const { id, buffer } = data;
              const item = bmsQueue.get(id);
              if (item) {
                bmsQueue.set(id, {
                  ...item,
                  resultBuffer: buffer,
                  progress: 100,
                  stage: "Completed",
                });
              }

              // Resolve the pending render promise
              const pending = pendingRenders.get(id);
              if (!pending) return;

              pending.resolve();
              pendingRenders.delete(id);
            }
            break;

          case MessageType.WARN:
            toast.warning(data.message);
            break;

          case MessageType.ERROR:
            {
              toast.error(data.error);

              if (!data.id) return;
              const pending = pendingRenders.get(data.id);
              if (!pending) return;

              pending.reject(data.error);
              pendingRenders.delete(data.id);
            }
            break;

          case MessageType.PROGRESS:
            {
              const { id, progress, stage } = data;
              const item = bmsQueue.get(id);
              if (!item) return;

              bmsQueue.set(id, { ...item, progress, stage });
            }
            break;
        }
      };

      renderWorker = worker;
      renderWorker.postMessage({ type: MessageType.INIT });
    };

    initWorker();

    // Cleanup on unmount
    return () => {
      createdUrls.forEach((url) => URL.revokeObjectURL(url));
      renderWorker?.terminate();
      renderWorker = null;
    };
  });

  async function startRender(id: string, item: BmsQueueItem): Promise<void> {
    if (!renderWorker || !workerReady) {
      toast.error("Renderer not ready");
      return;
    }

    if (!item.chart) {
      toast.error("No chart selected");
      return;
    }

    try {
      const chartFile = item.files.find((f) => f.name === item.chart);
      if (!chartFile) {
        throw new Error("Chart file not found");
      }

      const bmsText = await chartFile.text();

      // Create a promise that resolves when render completes
      const renderPromise = new Promise<void>((resolve, reject) => {
        pendingRenders.set(id, { resolve, reject });
      });

      renderWorker.postMessage({
        type: MessageType.RENDER,
        id,
        bmsText,
        useFloat32,
      });

      // Wait for render to complete
      await renderPromise;
    } catch (error) {
      toast.error(`Failed to render ${item.name}: ${error}`);
      pendingRenders.delete(id);
    }
  }

  let converting = $state(false);
  let compressing = $state(false);

  async function startAllRenders() {
    converting = true;
    await Promise.all([...bmsQueue.entries()].map(([id, item]) => startRender(id, item)));
    converting = false;
  }
</script>

<svelte:window {ondrop} {ondragover} />

<section class="flex w-full flex-1 flex-col justify-center gap-3">
  <div>
    <Dialog.Root bind:open={dialogOpen} onOpenChange={() => (itemToAdd = null)}>
      <Dialog.Trigger class={buttonVariants({ variant: "default" })}>
        <PlusIcon />Add BMS
      </Dialog.Trigger>
      <Dialog.Content class="sm:max-w-sm">
        <Dialog.Header>
          <Dialog.Title>Add BMS chart</Dialog.Title>
          <Dialog.Description>Select a BMS chart to add to convert queue.</Dialog.Description>
        </Dialog.Header>

        <div class="flex w-full flex-col gap-4 py-4">
          <div
            class="relative flex h-48 w-full flex-col items-center justify-center rounded-md border border-dashed p-4 text-center"
            bind:this={dropZone}
          >
            {#if !itemToAdd}
              <UploadIcon class="mb-2 h-8 w-8" />
              <p>Drop folder that contains BMS chart and assets here</p>
              <div class="text-muted-foreground">
                or <button
                  class="cursor-pointer text-sm text-primary underline"
                  onclick={() => {
                    let input = document.createElement("input");
                    input.type = "file";
                    input.webkitdirectory = true;
                    input.multiple = true;
                    input.onchange = (e) => {
                      const files = Array.from((e.target as HTMLInputElement).files!);
                      itemToAdd = new BmsQueueItem(
                        files[0].webkitRelativePath.split("/")[0]!,
                        files,
                      );
                    };
                    input.click();
                  }}>select folder</button
                >
              </div>
            {:else}
              <CheckIcon class="mb-2 h-8 w-8" />
              <p class="wrap-anywhere">
                {itemToAdd.name} selected
              </p>
            {/if}
          </div>

          <Select.Root
            type="single"
            name="chart"
            bind:value={selectedChart}
            disabled={!bmsFiles?.length}
          >
            <Select.Trigger class="w-full">
              {triggerContent}
            </Select.Trigger>
            <Select.Content>
              <Select.Group>
                {#each bmsFiles as chart (chart.name)}
                  <Select.Item value={chart.name} label={chart.name}>
                    {chart.name}
                  </Select.Item>
                {/each}
              </Select.Group>
            </Select.Content>
          </Select.Root>
        </div>

        <Dialog.Footer>
          <Button
            disabled={!selectedChart || !itemToAdd}
            onclick={() => {
              if (!itemToAdd || !selectedChart) return;

              itemToAdd.chart = selectedChart;

              // Check for duplicates
              const isDuplicate = Array.from(bmsQueue.values()).some(
                (existing) =>
                  existing.name === itemToAdd!.name && existing.chart === itemToAdd!.chart,
              );

              if (isDuplicate) {
                alert("This chart is already in the queue.");
                return;
              }

              bmsQueue.set(crypto.randomUUID(), itemToAdd);
              itemToAdd = null;
              dialogOpen = false;
            }}
          >
            <PlusIcon />Add
          </Button>
        </Dialog.Footer>
      </Dialog.Content>
    </Dialog.Root>
  </div>

  <Card class="flex flex-1 flex-row justify-between p-6">
    <!-- Queue -->
    <div class="flex flex-1 flex-col">
      <h3 class="pb-2 text-lg font-semibold">Queue</h3>
      <div class="flex flex-1 flex-col gap-2 overflow-y-auto">
        {#each bmsQueue.entries() as [id, item] (id)}
          <div class="flex w-full items-center justify-between">
            <div class="flex items-center gap-4">
              <Checkbox />
              <div class="flex flex-col">
                <p>{item.name}</p>
                <p class="text-sm text-muted-foreground">{item.chart}</p>
                {#if item.progress !== undefined && item.progress < 100}
                  <p class="text-xs text-muted-foreground">
                    {item.stage} - {item.progress}%
                  </p>
                {/if}
                {#if item.missingFiles && item.missingFiles.length > 0}
                  <p class="text-xs text-yellow-600">
                    ⚠️ {item.missingFiles.length} file(s) missing
                  </p>
                {/if}
              </div>
            </div>
            <div class="flex items-center gap-2">
              <Button
                variant="ghost"
                disabled={!item.resultBuffer}
                onclick={() => {
                  if (!item.resultBuffer) return;
                  const blob = new Blob([item.resultBuffer], {
                    type: "audio/wav",
                  });
                  const url = URL.createObjectURL(blob);
                  createdUrls.push(url);
                  const a = document.createElement("a");
                  a.href = url;
                  a.download = `${item.name} - ${item.chart}.wav`;
                  a.click();
                }}><DownloadIcon /></Button
              >
              <Button variant="ghost" onclick={() => bmsQueue.delete(id)}
                ><TrashIcon class="text-destructive" /></Button
              >
            </div>
          </div>
        {/each}
      </div>
    </div>

    <div class="w-px bg-border"></div>

    <!-- Settings -->
    <div class="flex flex-1 flex-col">
      <h3 class="pb-2 text-lg font-semibold">Settings</h3>
      <div class="flex flex-1 flex-col gap-2 overflow-y-auto">
        <div class="flex items-center justify-between">
          <div class="flex flex-col gap-1">
            <label for="float32" class="text-sm font-medium">Float32 Output</label>
            <p class="text-xs text-muted-foreground">
              Use 32-bit float format (larger files, higher precision)
            </p>
          </div>
          <Checkbox id="float32" bind:checked={useFloat32} />
        </div>
      </div>
    </div>
  </Card>

  <div class="flex justify-end gap-2">
    <Button
      variant="secondary"
      disabled={!bmsQueue.size ||
        !Array.from(bmsQueue.values()).every((item) => item.resultBuffer) ||
        compressing}
      onclick={async () => {
        const blobWriter = new BlobWriter();
        const zipWriter = new ZipWriter(blobWriter);

        compressing = true;
        await Promise.all(
          Array.from(bmsQueue.values()).map((item) => {
            if (!item.resultBuffer) return;
            return zipWriter.add(
              `${item.name} - ${item.chart}.wav`,
              new Uint8ArrayReader(new Uint8Array(item.resultBuffer)),
            );
          }),
        );

        zipWriter.close();
        const blob = await blobWriter.getData();
        const url = URL.createObjectURL(blob);
        createdUrls.push(url);
        const a = document.createElement("a");
        a.href = url;
        a.download = "output.zip";
        a.click();
        compressing = false;
      }}
    >
      {#if compressing}
        <Spinner />
      {:else}
        <DownloadIcon />
      {/if}
      Download all</Button
    >
    <Button onclick={startAllRenders} disabled={!workerReady || bmsQueue.size === 0 || converting}>
      {#if converting}
        <Spinner />
      {:else}
        <RefreshCwIcon />
      {/if}
      Convert
    </Button>
  </div>
</section>
