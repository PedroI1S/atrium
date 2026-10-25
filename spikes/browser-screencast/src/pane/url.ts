// Address-bar input -> URL. No search engine in this spike: anything that is not a URL or a
// host is treated as a path on the fixture server.

export function normalizeUrl(input: string, fixtureBase: string): string {
  const s = input.trim();
  if (s === "") return "about:blank";
  if (/^(about:|data:|file:)/i.test(s)) return s;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) return s;
  // A bare "form.html" is a fixture page, not a host.
  if (s.startsWith("/") || /^[\w-]+\.html?$/i.test(s)) return fixtureBase.replace(/\/$/, "") + (s.startsWith("/") ? s : "/" + s);
  const isLocal = /^(localhost|127\.\d+\.\d+\.\d+|\[::1\])(:\d+)?(\/|$)/i.test(s);
  if (isLocal) return "http://" + s;
  if (/^[^\s/]+\.[^\s/]+/.test(s) || /^[^\s/]+:\d+/.test(s)) return "https://" + s;
  return fixtureBase.replace(/\/$/, "") + "/" + s.replace(/^\/+/, "");
}

/** Short label for a tab or title: host+path without the fixture origin noise. */
export function displayUrl(url: string): string {
  return url.replace(/^https?:\/\//, "");
}
