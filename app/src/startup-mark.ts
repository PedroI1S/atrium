// The first module the bundle evaluates (index.tsx imports it first): the time the module
// script started, before its dependencies ran. With boot.js's mark before it and the
// "subscribe" stage after it, the startup profile splits fetching and parsing the bundle from
// evaluating it (M1-14b, L3).

declare global {
  interface Window { __atriumModuleStart?: number }
}

window.__atriumModuleStart = performance.timeOrigin + performance.now();

export {};
