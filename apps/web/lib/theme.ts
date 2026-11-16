export type Theme = "light" | "dark" | "system";

export const THEMES: { value: Theme; label: string }[] = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

const KEY = "cairn-theme";
const EVENT = "cairn-theme";

/**
 * Runs in <head> before first paint so the right class is on <html> with no flash. It also
 * follows the OS while the theme is System, and re-applies when setTheme fires the event.
 */
export const THEME_SCRIPT = `(function(){var d=document.documentElement,m=matchMedia("(prefers-color-scheme: dark)");function a(){var t;try{t=localStorage.getItem("${KEY}")}catch(e){}var k=t==="dark"||(t!=="light"&&m.matches);d.classList.toggle("dark",k)}a();m.addEventListener("change",a);addEventListener("${EVENT}",a)})()`;

export function readTheme(): Theme {
  try {
    const value = localStorage.getItem(KEY);
    return value === "light" || value === "dark" ? value : "system";
  } catch {
    return "system";
  }
}

export function setTheme(theme: Theme) {
  try {
    localStorage.setItem(KEY, theme);
  } catch {
    // Storage can be blocked; the choice then lasts until the page reloads.
  }
  dispatchEvent(new Event(EVENT));
}

export function subscribeTheme(onChange: () => void): () => void {
  addEventListener(EVENT, onChange);
  return () => removeEventListener(EVENT, onChange);
}
