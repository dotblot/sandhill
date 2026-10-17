// The thinking-effort control from draft 17: a rounded track with a dot at
// each level and a tick at the recommended one, and a brain for a knob. A
// fill grows behind the knob from nearly clear through pink to violet; at the
// top level the fill becomes a twinkling field of stars, the brain turns
// violet with light rays, and sparks burst from it. Nothing moves with
// reduced motion.

import { createEffect, createSignal, For, on, onCleanup } from "solid-js";
import { effortName } from "../model/agents";
import { Icon } from "./icons";

const HELP = "How long the model thinks before it answers. Higher effort handles harder problems, takes longer and uses more of your plan.";
const SPARKS = 16;
const SPARK_COLORS = ["#c8b8ff", "#a6d4ff", "#f0a8dc", "#ffffff", "#8f7cff"];
const STAR_COLORS = ["#241f5c", "#33299a", "#4a3cc8", "#6650e6", "#8a6cf2", "#ad8cf7", "#d1b0ff", "#f0a8dc", "#a6d4ff", "#ffffff"];
/** Star squares: 3px with a 1px gap. */
const STAR_STEP = 4;
const TWINKLE_MS = 120;

export function EffortControl(props: {
  /** The levels, lowest first. */
  levels: string[];
  /** The current level; undefined shows the recommended one. */
  value: string | undefined;
  recommended: string;
  onChange: (level: string) => void;
  /** Shown but not changeable, with this explanation. */
  disabled?: string;
}) {
  const top = () => props.levels.length - 1;
  const index = () => {
    const i = props.levels.indexOf(props.value ?? props.recommended);
    return i < 0 ? Math.max(0, props.levels.indexOf(props.recommended)) : i;
  };
  const level = () => props.levels[index()];
  const galaxy = () => index() === top();
  const fraction = (i: number) => (top() > 0 ? i / top() : 0);
  // The 30px knob's centre stops 15px from each end.
  const x = (i: number) => `calc(15px + (100% - 30px) * ${fraction(i)})`;
  const recommendedAt = () => Math.max(0, props.levels.indexOf(props.recommended));

  let track!: HTMLDivElement;
  let name!: HTMLSpanElement;
  let knob!: HTMLSpanElement;
  let stars!: HTMLCanvasElement;
  const [twinkling, setTwinkling] = createSignal(false);

  const set = (i: number) => {
    if (props.disabled) return;
    const next = Math.max(0, Math.min(top(), i));
    if (next !== index()) props.onChange(props.levels[next]);
  };
  const fromX = (clientX: number) => {
    const box = track.getBoundingClientRect();
    return Math.round(Math.min(1, Math.max(0, (clientX - box.left - 15) / (box.width - 30))) * top());
  };
  const onPointerDown = (e: PointerEvent) => {
    if (props.disabled) return;
    track.setPointerCapture(e.pointerId);
    set(fromX(e.clientX));
    const move = (ev: PointerEvent) => set(fromX(ev.clientX));
    const up = () => {
      track.removeEventListener("pointermove", move);
      track.removeEventListener("pointerup", up);
    };
    track.addEventListener("pointermove", move);
    track.addEventListener("pointerup", up);
  };
  const onKeyDown = (e: KeyboardEvent) => {
    const step = { ArrowLeft: -1, ArrowDown: -1, ArrowRight: 1, ArrowUp: 1 }[e.key];
    if (step) set(index() + step);
    else if (e.key === "Home") set(0);
    else if (e.key === "End") set(top());
    else return;
    e.preventDefault();
  };

  // The name blurs in when it changes; reaching the top bursts sparks, and
  // leaving it flashes the brain.
  createEffect(
    on(
      index,
      (now, before) => {
        if (before === undefined) return;
        replay(name, "blurin");
        if (now === top()) sparks(knob);
        else if (before === top()) replay(knob, "flash");
      },
      { defer: false },
    ),
  );
  createEffect(() => setTwinkling(galaxy()));
  createEffect(on(twinkling, (on) => on && starField(stars, track, () => twinkling())));
  onCleanup(() => setTwinkling(false));

  return (
    <div class="eff" classList={{ off: !!props.disabled }} title={props.disabled}>
      <div class="eff-h">
        <span class="eff-k">Effort</span>
        <span ref={name} class="eff-v" classList={{ galaxy: galaxy() }}>
          {effortName(level())}
        </span>
        <span class="eff-help" title={HELP}>
          <Icon name="help" />
        </span>
      </div>
      <div class="eff-ends">
        <span>Faster</span>
        <span>Smarter</span>
      </div>
      <div
        ref={track}
        class="eff-track"
        classList={{ galaxy: galaxy() }}
        role="slider"
        tabindex={props.disabled ? -1 : 0}
        aria-label="Thinking effort"
        aria-valuemin={0}
        aria-valuemax={top()}
        aria-valuenow={index()}
        aria-valuetext={effortName(level())}
        aria-disabled={props.disabled ? "true" : undefined}
        style={{ "--f": String(fraction(index())) }}
        onPointerDown={onPointerDown}
        onKeyDown={onKeyDown}
      >
        <span class="eff-fill" />
        <canvas ref={stars} class="eff-stars" aria-hidden="true" />
        <For each={props.levels}>
          {(l, i) => <span class="eff-stop" classList={{ rec: l === props.recommended }} style={{ left: x(i()) }} />}
        </For>
        <span ref={knob} class="eff-knob">
          <span class="eff-rays" />
          <Brain />
        </span>
      </div>
      <div class="eff-rec">
        <span style={{ left: x(recommendedAt()) }}>Recommended</span>
      </div>
    </div>
  );
}

/** Two bumpy halves of a brain, drawn at 20px and shown at 30px. */
function Brain() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path
        class="b-fill"
        d="M9.2 3.2C8.4 2.4 7 2.4 6.3 3.2 5.2 3 4.2 3.8 4.3 4.9 3.3 5.3 2.9 6.4 3.4 7.3 2.6 7.9 2.5 9.1 3.2 9.8 2.6 10.6 2.8 11.8 3.6 12.3 3.4 13.4 4.2 14.4 5.3 14.4 5.6 15.5 6.8 16.1 7.8 15.7 8.3 16.4 8.8 16.7 9.2 16.6ZM10.8 3.2C11.6 2.4 13 2.4 13.7 3.2 14.8 3 15.8 3.8 15.7 4.9 16.7 5.3 17.1 6.4 16.6 7.3 17.4 7.9 17.5 9.1 16.8 9.8 17.4 10.6 17.2 11.8 16.4 12.3 16.6 13.4 15.8 14.4 14.7 14.4 14.4 15.5 13.2 16.1 12.2 15.7 11.7 16.4 11.2 16.7 10.8 16.6Z"
      />
      <path class="b-line" d="M9.2 7.4H7.7M9.2 11.3H7.9M10.8 7.4h1.5M10.8 11.3h1.3" />
    </svg>
  );
}

function stillMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/** Runs a CSS animation class again. */
function replay(el: Element, cls: string) {
  el.classList.remove(cls);
  void (el as HTMLElement).offsetWidth;
  el.classList.add(cls);
}

/**
 * Fills the canvas with small squares in purples, blues, pinks and white,
 * brighter toward the knob, and re-colours some every few frames while
 * `going()` holds.
 */
function starField(canvas: HTMLCanvasElement, track: HTMLElement, going: () => boolean) {
  // The canvas, not the track: it is the thinner bar inside the track.
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  const g = canvas.getContext?.("2d");
  if (!g) return;
  // Not laid out yet, as in a popover just opened: try once it is.
  if (!w || !h) {
    requestAnimationFrame(() => going() && canvas.clientWidth && starField(canvas, track, going));
    return;
  }
  const dpr = window.devicePixelRatio || 1;
  canvas.width = w * dpr;
  canvas.height = h * dpr;
  g.scale(dpr, dpr);
  const cols = Math.floor((w - 2) / STAR_STEP);
  const rows = Math.floor(h / STAR_STEP);
  const top = (h - rows * STAR_STEP + 1) / 2;
  const pick = (c: number) => {
    const t = c / cols;
    return Math.min(1, Math.random() * (0.2 + 0.8 * t) + 0.12 * t);
  };
  const lum = Array.from({ length: cols }, (_, c) => Array.from({ length: rows }, () => pick(c)));
  const draw = () => {
    g.fillStyle = "#120f2c";
    g.fillRect(0, 0, w, h);
    for (let c = 0; c < cols; c++) {
      for (let r = 0; r < rows; r++) {
        const v = lum[c][r];
        g.globalAlpha = 0.3 + 0.7 * v;
        g.fillStyle = STAR_COLORS[Math.min(STAR_COLORS.length - 1, Math.floor(v * STAR_COLORS.length))];
        g.fillRect(1 + c * STAR_STEP, top + r * STAR_STEP, STAR_STEP - 1, STAR_STEP - 1);
      }
    }
    g.globalAlpha = 1;
  };
  draw();
  if (stillMotion()) return;
  const tick = () => {
    if (!going() || !canvas.isConnected) return;
    for (let k = 0; k < cols * rows * 0.08; k++) {
      const c = Math.floor(Math.random() * cols);
      lum[c][Math.floor(Math.random() * rows)] = pick(c);
    }
    draw();
    setTimeout(() => requestAnimationFrame(tick), TWINKLE_MS);
  };
  setTimeout(() => requestAnimationFrame(tick), TWINKLE_MS);
}

/** A burst of small squares flying out of the knob. */
function sparks(knob: HTMLElement) {
  if (stillMotion()) return;
  for (let k = 0; k < SPARKS; k++) {
    const spark = document.createElement("i");
    const angle = Math.random() * Math.PI * 2;
    const distance = 12 + Math.random() * 14;
    spark.className = "eff-spark";
    spark.style.background = SPARK_COLORS[k % SPARK_COLORS.length];
    spark.style.animationDelay = `${Math.round(Math.random() * 120)}ms`;
    spark.style.setProperty("--dx", `${(Math.cos(angle) * distance).toFixed(1)}px`);
    spark.style.setProperty("--dy", `${(Math.sin(angle) * distance).toFixed(1)}px`);
    knob.append(spark);
    setTimeout(() => spark.remove(), 1200);
  }
}
