import { render } from "solid-js/web";
import { createShell } from "./backend";
import { Workbench } from "./ui/Workbench";
import "./fonts/maple-mono.css";
import "./styles.css";

// The Mac app draws its own title bar, with the window buttons over it.
if ("__TAURI_INTERNALS__" in window && /Mac/.test(navigator.userAgent)) {
  document.documentElement.dataset.frame = "mac";
}

const shell = await createShell();
const initial = await shell.context();
render(() => <Workbench shell={shell} initial={initial} />, document.getElementById("root")!);
