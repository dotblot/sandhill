import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import type { LinkState } from "../backend";
import type { PingReply, SessionInfo } from "../backend/types";
import type { Workspace } from "../model/workspace";
import { ConnectionItem, destination, duration } from "./Connection";

const settle = () => new Promise((r) => setTimeout(r, 0));

function setup(host: string, ping: Partial<PingReply>, since?: number) {
  const [link, setLink] = createSignal<LinkState>({ status: "connected" });
  const done: string[] = [];
  const ws = {
    backend: {
      host,
      reconnect: async () => void done.push("reconnect"),
      disconnect: async () => void done.push("disconnect"),
    },
    link,
    host: () => ({ protocol_version: 1, daemon_version: "0.1.0", home: "/home/huan", hostname: "gpu01", ...ping }),
    connectedSince: () => since,
    liveSessions: () => [{ id: "s1" } as SessionInfo],
    call: async (method: string) => void done.push(method),
  } as unknown as Workspace;
  const root = document.body.appendChild(document.createElement("div"));
  const dispose = render(() => <ConnectionItem ws={ws} />, root);
  const item = root.querySelector(".status-item.conn") as HTMLElement;
  const pop = () => document.querySelector(".connpop");
  const rows = () => [...pop()!.querySelectorAll("dt")].map((dt) => `${dt.textContent}: ${dt.nextElementSibling?.textContent}`);
  const actions = () => [...pop()!.querySelectorAll("button.mm-i")].map((b) => b.textContent);
  const action = (label: string) => [...pop()!.querySelectorAll("button.mm-i")].find((b) => b.textContent === label) as HTMLElement;
  return { item, pop, rows, actions, action, done, setLink, dispose: () => (dispose(), root.remove()) };
}

describe("ConnectionItem", () => {
  it("shows a server's icon, name and connection, with Reconnect and Disconnect", async () => {
    const t = setup("huan@gpu01", { user: "huan", started_by: "app" }, Date.now() - (3 * 60 + 12) * 60_000);
    expect(t.item.textContent).toBe("huan@gpu01");
    expect(t.item.title).toBe("huan@gpu01 · sandhilld 0.1.0 · connected");
    expect(t.item.querySelector(".conn-ico")?.className).toBe("conn-ico");
    t.item.click();
    await settle();
    expect(t.pop()?.querySelector(".kp-h")?.textContent).toBe("huan@gpu01connected");
    expect(t.rows().slice(0, 4)).toEqual([
      "Address: gpu01",
      "Via: ssh, as huan",
      "sandhilld: 0.1.0 · started by Sandhill",
      "Connected for: 3 h 12 min",
    ]);
    expect(t.rows()).toContain("Kernels: 1 running");
    expect(t.rows().some((r) => /^Latency: \d+ ms$/.test(r))).toBe(true);
    expect(t.actions()).toEqual(["Reconnect", "Disconnect", "Open the sandhilld log"]);
    t.action("Disconnect").click();
    expect(t.done).toContain("disconnect");
    expect(t.pop()).toBeNull();
    t.dispose();
  });

  it("turns red with a slash when disconnected, and offers Connect", async () => {
    const t = setup("gpu01", {});
    t.setLink({ status: "disconnected", stopped: true });
    expect(t.item.classList.contains("off")).toBe(true);
    expect(t.item.querySelector(".conn-ico")?.classList.contains("off")).toBe(true);
    t.item.click();
    await settle();
    expect(t.rows().some((r) => r.startsWith("Kernels"))).toBe(false);
    expect(t.actions()).toEqual(["Connect", "Open the sandhilld log"]);
    t.action("Connect").click();
    expect(t.done).toContain("reconnect");
    // Reconnecting: orange and pulsing.
    t.setLink({ status: "connecting", error: "lost the connection to sandhilld" });
    expect(t.item.querySelector(".conn-ico")?.classList.contains("busy")).toBe(true);
    expect(t.item.title).toBe("gpu01 · sandhilld 0.1.0 · reconnecting");
    t.dispose();
  });

  it("shows this Mac's daemon, with Restart sandhilld", async () => {
    const t = setup("local", { started_by: "launchd", started_ms: Date.now() - 50 * 3600_000 });
    expect(t.item.textContent).toBe("Local");
    t.item.click();
    await settle();
    expect(t.pop()?.querySelector(".kp-h")?.textContent).toBe("This Macrunning");
    expect(t.rows().slice(0, 2)).toEqual(["sandhilld: 0.1.0 · launchd agent", "Running for: 2 d 2 h"]);
    expect(t.actions()).toEqual(["Restart sandhilld", "Open the sandhilld log"]);
    t.action("Restart sandhilld").click();
    await settle();
    expect(t.done).toContain("stop_daemon");
    t.dispose();
  });
});

describe("connection helpers", () => {
  it("splits an ssh destination", () => {
    expect(destination("me@gpu01.lab.org")).toEqual({ user: "me", address: "gpu01.lab.org" });
    expect(destination("lab")).toEqual({ address: "lab" });
  });

  it("says how long, roughly", () => {
    expect(duration(40_000)).toBe("40 s");
    expect(duration(12 * 60_000)).toBe("12 min");
    expect(duration((3 * 60 + 12) * 60_000)).toBe("3 h 12 min");
    expect(duration(52 * 3600_000)).toBe("2 d 4 h");
  });
});
