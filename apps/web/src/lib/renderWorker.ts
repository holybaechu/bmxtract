import { simd, threads } from "wasm-feature-detect";
import { type Message, MessageType } from "./types";
import log from "loglevel";
import {
  createWorkerMessenger,
  createFileCacheManager,
  createGetManyBytes,
  concatenateChunks,
} from "./utils/workerHelpers";

log.debug("Started worker.");

const sw = self as unknown as {
  onmessage: (ev: MessageEvent<Message>) => void;
  navigator?: { hardwareConcurrency?: number };
};

let renderFn:
  | ((
      bms_text: string,
      use_float32: boolean,
      get_many_bytes: (paths: string[]) => Promise<(Uint8Array | undefined)[]>,
      on_chunk: (u8: Uint8Array) => void,
      on_progress: (progress: number, stage: string) => void,
    ) => Promise<void>)
  | null = null;

const postMessage = createWorkerMessenger();
const cacheManager = createFileCacheManager();

const recommendedBrowsersMessage =
  "Update your browser to the latest version\nor try using Chrome, Firefox or Edge.";

sw.onmessage = async (ev: MessageEvent<Message>) => {
  log.debug("Received message", ev.data);

  switch (ev.data.type) {
    case MessageType.INIT:
      try {
        const { default: init, initThreadPool, convert_bms_to_wav } = await import("@bmxtract/lib");

        if (!(await simd())) {
          return postMessage({
            type: MessageType.ERROR,
            error: "This browser does not support SIMD.\n" + recommendedBrowsersMessage,
          });
        }

        await init();
        renderFn = convert_bms_to_wav;

        if (!(await threads())) {
          postMessage({
            type: MessageType.WARN,
            message:
              "Thread support not detected. Using single thread.\n" + recommendedBrowsersMessage,
          });
        } else {
          try {
            await initThreadPool(sw.navigator?.hardwareConcurrency ?? 1);
            log.debug("Thread pool initialized.");
          } catch (err) {
            log.warn("Failed to initialize thread pool.", err);
            postMessage({
              type: MessageType.WARN,
              message: "Failed to initialize thread pool. Using single thread.",
            });
          }
        }

        log.debug("Worker initialized.");
        postMessage({ type: MessageType.INIT });
      } catch (err) {
        log.error("Unknown error has occurred while initializing the worker.", err);
        return postMessage({
          type: MessageType.ERROR,
          error: "Unknown error has occurred while initializing the worker.",
        });
      }

      break;

    case MessageType.READ_FILES_RESPONSE:
      {
        const { id, buffers } = ev.data;
        const result = cacheManager.resolvePendingRequest(id, buffers);
        if (!result) {
          log.warn(`No pending file read request for ID: ${id}`);
        }
      }
      break;

    case MessageType.RENDER:
      {
        const { id, bmsText, useFloat32 } = ev.data;

        if (!renderFn) {
          return postMessage({
            type: MessageType.ERROR,
            id,
            error: "Renderer not initialized.",
          });
        }

        try {
          const sessionCache = cacheManager.createSession(id);
          const chunks: Uint8Array[] = [];

          const getManyBytes = createGetManyBytes(
            id,
            sessionCache,
            postMessage,
            cacheManager.setPendingRequest.bind(cacheManager),
          );

          const onChunk = (u8: Uint8Array) => {
            chunks.push(u8);
          };

          const onProgress = (progress: number, stage: string) => {
            postMessage({
              type: MessageType.PROGRESS,
              id,
              progress,
              stage,
            });
          };

          await renderFn(bmsText, useFloat32, getManyBytes, onChunk, onProgress);

          const buffer = concatenateChunks(chunks);
          postMessage({ type: MessageType.RESULT, id, buffer }, [buffer]);

          cacheManager.deleteSession(id);
        } catch (err) {
          log.error("Unknown error has occurred while rendering.", err);
          cacheManager.deleteSession(id);
          return postMessage({
            type: MessageType.ERROR,
            id,
            error: `Unknown error has occurred while rendering: ${err}`,
          });
        }
      }
      break;

    default:
      log.warn("Unknown message type received.", ev.data);
      break;
  }
};
