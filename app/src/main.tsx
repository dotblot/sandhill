import { render } from "solid-js/web";
import { createShell } from "./backend";
import { Workspace } from "./model/workspace";
import { App } from "./ui/App";
import { StartScreen } from "./ui/StartScreen";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/700.css";
import "./styles.css";

// The Mac app draws its own title bar, with the window buttons over it.
if ("__TAURI_INTERNALS__" in window && /Mac/.test(navigator.userAgent)) {
  document.documentElement.dataset.frame = "mac";
}

const shell = await createShell();
const context = await shell.context();
const root = document.getElementById("root")!;
if (context.kind === "project") {
  const ws = new Workspace(await shell.connect(context.host), context.path);
  render(() => <App ws={ws} shell={shell} project={context.path} />, root);
} else {
  const workspace = async (host: string) => new Workspace(await shell.connect(host));
  render(() => <StartScreen shell={shell} workspace={workspace} />, root);
}
