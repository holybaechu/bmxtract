import type { Handle } from "@sveltejs/kit";

export const handle: Handle = async ({ event, resolve }) => {
  const response = await resolve(event, {
    filterSerializedResponseHeaders: (name) =>
      name === "cross-origin-embedder-policy" || name === "cross-origin-opener-policy",
  });

  if (response.headers.get("content-type")?.includes("application/wasm")) {
    response.headers.set("Cross-Origin-Opener-Policy", "same-origin");
    response.headers.set("Cross-Origin-Embedder-Policy", "require-corp");
  }

  return response;
};
