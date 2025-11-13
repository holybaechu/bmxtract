import { MessageType, type Message } from "$lib/types";

/**
 * Type-safe postMessage wrapper for worker
 */
export function createWorkerMessenger() {
  const sw = self as unknown as {
    postMessage: (message: unknown, transfer?: Transferable[]) => void;
  };

  return (msg: Message, transfer?: Transferable[]) => {
    if (transfer) {
      sw.postMessage(msg, transfer);
    } else {
      sw.postMessage(msg);
    }
  };
}

/**
 * Creates a file cache manager
 */
export function createFileCacheManager() {
  type FileReadResolver = (value: (Uint8Array | undefined)[]) => void;
  const pendingFileReads = new Map<string, FileReadResolver>();
  const renderCaches = new Map<string, Map<string, Uint8Array>>();

  return {
    createSession(id: string) {
      const sessionCache = new Map<string, Uint8Array>();
      renderCaches.set(id, sessionCache);
      return sessionCache;
    },

    deleteSession(id: string) {
      renderCaches.delete(id);
    },

    setPendingRequest(id: string, resolver: FileReadResolver) {
      pendingFileReads.set(id, resolver);
    },

    resolvePendingRequest(id: string, buffers: (ArrayBuffer | undefined)[]) {
      const resolver = pendingFileReads.get(id);
      if (!resolver) {
        return null;
      }
      const result = buffers.map((b) => (b ? new Uint8Array(b) : undefined));
      resolver(result);
      pendingFileReads.delete(id);
      return result;
    },
  };
}

/**
 * Creates the getManyBytes function for a render session
 */
export function createGetManyBytes(
  id: string,
  sessionCache: Map<string, Uint8Array>,
  postMessage: (msg: Message) => void,
  setPendingRequest: (id: string, resolver: (value: (Uint8Array | undefined)[]) => void) => void,
) {
  return async (paths: string[]): Promise<(Uint8Array | undefined)[]> => {
    const missing: string[] = [];
    const indices: number[] = [];
    const result: (Uint8Array | undefined)[] = new Array(paths.length).fill(undefined);

    // Check cache first
    for (let i = 0; i < paths.length; i++) {
      const p = paths[i];
      const cached = sessionCache.get(p);
      if (cached) {
        result[i] = cached;
      } else {
        missing.push(p);
        indices.push(i);
      }
    }

    // Request missing files from main thread
    if (missing.length) {
      const wait = new Promise<(Uint8Array | undefined)[]>((resolve) => {
        setPendingRequest(id, resolve);
      });

      postMessage({
        type: MessageType.READ_FILES,
        id,
        paths: missing,
      });

      const received = await wait;

      // Populate result and cache
      for (let j = 0; j < received.length; j++) {
        const idx = indices[j];
        const buf = received[j];
        result[idx] = buf;
        const name = paths[idx];
        if (buf) sessionCache.set(name, buf);
      }
    }

    return result;
  };
}

/**
 * Concatenates Uint8Array chunks into a single buffer
 */
export function concatenateChunks(chunks: Uint8Array[]): ArrayBuffer {
  const totalLen = chunks.reduce((sum, c) => sum + c.byteLength, 0);
  const out = new Uint8Array(totalLen);
  let off = 0;
  for (const c of chunks) {
    out.set(c, off);
    off += c.byteLength;
  }
  return out.buffer;
}
