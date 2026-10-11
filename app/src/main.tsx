import { render } from "solid-js/web";
import { createShell } from "./backend";
import { Workspace } from "./model/workspace";
import { App } from "./ui/App";
import { StartScreen } from "./ui/StartScreen";
import "./styles.css";

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
