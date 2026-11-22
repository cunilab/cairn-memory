const CHANGED = "cairn:memories-changed";

/** Tells every mounted list that a memory was forgotten, so it can drop or reload its rows. */
export function announceMemoriesChanged() {
  window.dispatchEvent(new Event(CHANGED));
}

export function onMemoriesChanged(listener: () => void): () => void {
  window.addEventListener(CHANGED, listener);
  return () => window.removeEventListener(CHANGED, listener);
}
