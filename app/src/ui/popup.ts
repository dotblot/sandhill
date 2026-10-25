import { onCleanup } from "solid-js";

/**
 * Calls `close` on a click outside the pop-up and its anchor, or on Escape.
 * Either may be a function, for elements that don't exist yet.
 */
export function closeOnOutside(
  popup: () => HTMLElement | undefined,
  anchor: HTMLElement | (() => HTMLElement | undefined),
  close: () => void,
) {
  const outside = (e: MouseEvent) => {
    const target = e.target as Node;
    const at = typeof anchor === "function" ? anchor() : anchor;
    if (!popup()?.contains(target) && !at?.contains(target)) close();
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && close();
  document.addEventListener("mousedown", outside);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", outside);
    document.removeEventListener("keydown", escape);
  });
}
