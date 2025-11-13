import type { Handle } from "@sveltejs/kit";

export const handle: Handle = async ({ event, resolve }) => {
  const response = await resolve(event, {
    filterSerializedResponseHeaders: (name) =>
      name === "cross-origin-embedder-policy" || name === "cross-origin-opener-policy",
  });

  response.headers.set("Cross-Origin-Opener-Policy", "same-origin");
  response.headers.set("Cross-Origin-Embedder-Policy", "require-corp");

  return response;
};
