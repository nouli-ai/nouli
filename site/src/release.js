// Resolve the published release when the page is viewed, as Handshake does.
export async function showLatestRelease() {
  const label = document.querySelector("[data-latest-release]");
  if (!label) return;

  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10000);
  try {
    const response = await fetch("https://api.github.com/repos/nouli-ai/nouli/releases/latest", {
      headers: { Accept: "application/vnd.github+json" },
      cache: "no-store",
      signal: controller.signal,
    });
    if (!response.ok) throw new Error("Release lookup failed");
    const release = await response.json();
    if (typeof release.tag_name !== "string" || !release.tag_name.trim()) throw new Error("Missing release tag");
    label.textContent = release.tag_name;
  } catch {
    // Keep the generic latest-release link when GitHub cannot be reached.
  } finally {
    clearTimeout(timeout);
  }
}
