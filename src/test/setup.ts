// The dev shell may export NODE_ENV=production, which makes React load its
// production build (no `act`). Force test mode before React is first imported;
// test files import React only after this setup file has run.
process.env.NODE_ENV = "test";

const { cleanup } = await import("@testing-library/react");
await import("@testing-library/jest-dom/vitest");

afterEach(() => cleanup());

if (!window.matchMedia) {
  window.matchMedia = (query: string) =>
    ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }) as MediaQueryList;
}

if (!globalThis.crypto?.randomUUID) {
  let n = 0;
  Object.defineProperty(globalThis, "crypto", {
    value: { ...globalThis.crypto, randomUUID: () => `00000000-0000-4000-8000-${String(++n).padStart(12, "0")}` },
  });
}

export {};
