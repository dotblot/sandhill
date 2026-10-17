import { onCleanup } from "solid-js";

/** Calls `close` on a click outside the pop-up and its anchor, or on Escape. */
export function closeOnOutside(popup: () => HTMLElement, anchor: HTMLElement, close: () => void) {
  const outside = (e: MouseEvent) => {
    const target = e.target as Node;
    if (!popup().contains(target) && !anchor.contains(target)) close();
  };
  const escape = (e: KeyboardEvent) => e.key === "Escape" && close();
  document.addEventListener("mousedown", outside);
  document.addEventListener("keydown", escape);
  onCleanup(() => {
    document.removeEventListener("mousedown", outside);
    document.removeEventListener("keydown", escape);
  });
}
