import { Icon } from "./icons";

export type Pane = "editor" | "console" | "agents";

/** Minimize and maximize, at the right end of a pane's tab strip. */
export function PaneButtons(props: {
  pane: Pane;
  min: boolean;
  max: boolean;
  onMin: (p: Pane) => void;
  onMax: (p: Pane) => void;
}) {
  return (
    <span class="pwin">
      <button title={props.min ? "Restore" : "Minimize"} onClick={() => props.onMin(props.pane)}>
        <Icon name={props.min ? "restore" : "minimize"} small />
      </button>
      <button title={props.max ? "Restore" : "Maximize"} onClick={() => props.onMax(props.pane)}>
        <Icon name={props.max ? "restore" : "maximize"} small />
      </button>
    </span>
  );
}
