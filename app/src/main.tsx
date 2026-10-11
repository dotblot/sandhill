import { render } from "solid-js/web";
import { createBackend } from "./backend";
import { Workspace } from "./model/workspace";
import { App } from "./ui/App";
import "./styles.css";

const backend = await createBackend();
const ws = new Workspace(backend);
render(() => <App ws={ws} />, document.getElementById("root")!);
