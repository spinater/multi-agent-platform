"use client";

import { useEffect, useRef, type RefObject } from "react";

import { activityMarks, prefersReducedMotion } from "@/lib/activity";
import { isHeld } from "@/lib/intake";

import type { Graph, GraphNode, NodeType } from "./model";
import { ORBIT_RINGS, depthStep, graphColumns, hash, layout, type OrbitLayer, type View } from "./layouts";

/** Imperative controls for the toolbar: reset the camera, or fly to a node. */
export type CanvasControls = { fit: () => void; fly: (id: string) => void };

export type CanvasProps = {
  graph: Graph;
  view: View;
  all: boolean;
  /** The orchestrator's core glows when it is running. */
  running: boolean;
  selected: string | null;
  onSelect: (id: string | null) => void;
  /** `all`, or a group key to isolate. */
  filter: string;
  hidden: Partial<Record<NodeType, boolean>>;
  names: boolean;
  motion: boolean;
  search: string;
  theme: string;
  controlsRef: RefObject<CanvasControls | null>;
};

type Live = GraphNode & {
  x: number;
  y: number;
  z: number;
  a: number;
  tx: number;
  ty: number;
  tz: number;
  ta: number;
  sx: number;
  sy: number;
  ss: number;
  sz: number;
  fa: number;
  da: number;
  vis: boolean;
  match: boolean;
};

const SESSION_TONE: Record<string, string> = {
  starting: "--info",
  working: "--ok",
  idle: "--idle",
  waiting_input: "--accent",
  rate_limited: "--warn",
  needs_auth: "--bad",
  needs_trust: "--warn",
  error: "--bad",
  stalled: "--warn",
  exited: "--idle",
};
const HIT: Record<NodeType, number> = { orch: 18, group: 14, stage: 12, session: 9, task: 9, run: 5 };
/** The circle view looks straight down on the orbit's rings. */
const CIRCLE_PITCH = Math.PI / 2;
const LINK_ALPHA = { hub: 0.3, own: 0.16, work: 0.5, dep: 0.22, stage: 0.5, infra: 0.12 } as const;

/** The command centre's canvas (design: `draw()` in Agent Command Centre.dc.html): nodes ease towards their
 * layout targets, the orbit camera turns slowly, active links carry moving dots. Drag orbits (orbit view) or pans
 * (graph view), the wheel zooms, a click selects. */
class Engine {
  cv: HTMLCanvasElement;
  ctx: CanvasRenderingContext2D;
  props: CanvasProps;
  nodes = new Map<string, Live>();
  cam = { yaw: 0.6, pitch: 0.55, yawT: 0.6, pitchT: 0.55, R: 300, zoom: 1, zoomT: 1, px: 0, py: 0, pxT: 0, pyT: 0, ox: 0 };
  ga: Record<View, number> = { orbit: 0, circle: 0, graph: 0 };
  W = 0;
  H = 0;
  dpr = 1;
  hover: string | null = null;
  dragging = false;
  moved = 0;
  lx = 0;
  ly = 0;
  flyTo: string | null = null;
  flyN = 0;
  raf = 0;
  colors = new Map<string, string>();
  fonts = { mono: "ui-monospace, monospace", sans: "ui-sans-serif, sans-serif" };
  lastGraph: Graph | null = null;
  lastView: View | null = null;
  lastTheme = "";
  cleanup: (() => void)[] = [];
  born = performance.now();
  frames = 0;
  /** The system asks for less motion: spawn bursts, ripples and shakes become fading rings in place. */
  reduce = false;

  constructor(cv: HTMLCanvasElement, props: CanvasProps) {
    this.cv = cv;
    this.ctx = cv.getContext("2d")!;
    this.props = props;
    this.ga[props.view] = 1;
    if (props.view === "circle") this.cam.pitch = this.cam.pitchT = CIRCLE_PITCH;
    else if (props.view !== "orbit") {
      this.cam.pitch = this.cam.pitchT = 0;
      this.cam.yaw = this.cam.yawT = 0;
    }
    const resize = () => {
      const r = cv.parentElement!.getBoundingClientRect();
      this.dpr = Math.min(2, window.devicePixelRatio || 1);
      this.W = r.width;
      this.H = r.height;
      cv.width = Math.round(r.width * this.dpr);
      cv.height = Math.round(r.height * this.dpr);
    };
    this.reduce = prefersReducedMotion();
    let mq: MediaQueryList | null = null;
    const onMotion = (e: MediaQueryListEvent) => {
      this.reduce = e.matches;
    };
    try {
      mq = window.matchMedia("(prefers-reduced-motion: reduce)");
      mq.addEventListener("change", onMotion);
    } catch {
      // No matchMedia: keep the default.
    }
    this.cleanup.push(() => mq?.removeEventListener("change", onMotion));
    const ro = new ResizeObserver(resize);
    ro.observe(cv.parentElement!);
    resize();
    const down = (e: PointerEvent) => {
      this.dragging = true;
      this.moved = 0;
      this.lx = e.clientX;
      this.ly = e.clientY;
      this.flyTo = null;
      try {
        cv.setPointerCapture(e.pointerId);
      } catch {
        // Synthetic events have no capturable pointer.
      }
    };
    const move = (e: PointerEvent) => {
      const r = cv.getBoundingClientRect();
      if (this.dragging) {
        const dx = e.clientX - this.lx;
        const dy = e.clientY - this.ly;
        this.lx = e.clientX;
        this.ly = e.clientY;
        this.moved += Math.abs(dx) + Math.abs(dy);
        if (this.props.view === "orbit") {
          this.cam.yawT += dx * 0.006;
          this.cam.yaw += dx * 0.006;
          this.cam.pitchT = Math.max(-1.2, Math.min(1.2, this.cam.pitchT + dy * 0.006));
        } else {
          this.cam.pxT += dx;
          this.cam.pyT += dy;
          this.cam.px += dx;
          this.cam.py += dy;
        }
      }
      this.hover = this.dragging && this.moved > 3 ? null : this.hit(e.clientX - r.left, e.clientY - r.top);
      cv.style.cursor = this.dragging && this.moved > 3 ? "grabbing" : this.hover ? "pointer" : "grab";
    };
    const up = (e: PointerEvent) => {
      if (this.dragging && this.moved < 4) {
        const r = cv.getBoundingClientRect();
        this.props.onSelect(this.hit(e.clientX - r.left, e.clientY - r.top));
      }
      this.dragging = false;
    };
    const leave = () => {
      this.hover = null;
    };
    const wheel = (e: WheelEvent) => {
      e.preventDefault();
      this.cam.zoomT = Math.max(0.45, Math.min(3.2, this.cam.zoomT * Math.exp(-e.deltaY * 0.0012)));
    };
    cv.addEventListener("pointerdown", down);
    cv.addEventListener("pointermove", move);
    cv.addEventListener("pointerup", up);
    cv.addEventListener("pointerleave", leave);
    cv.addEventListener("wheel", wheel, { passive: false });
    const frame = (time: number) => {
      this.raf = requestAnimationFrame(frame);
      if (!this.W || !this.H) resize();
      if (this.W) this.draw(time);
    };
    this.raf = requestAnimationFrame(frame);
    this.cleanup.push(
      () => cancelAnimationFrame(this.raf),
      () => ro.disconnect(),
      () => {
        cv.removeEventListener("pointerdown", down);
        cv.removeEventListener("pointermove", move);
        cv.removeEventListener("pointerup", up);
        cv.removeEventListener("pointerleave", leave);
        cv.removeEventListener("wheel", wheel);
      },
    );
    this.update(props);
  }

  destroy() {
    this.cleanup.forEach((f) => f());
  }

  update(props: CanvasProps) {
    const prevView = this.lastView;
    this.props = props;
    if (props.theme !== this.lastTheme) {
      this.lastTheme = props.theme;
      this.colors.clear();
      // The shell sets `data-theme` in its own effect, which runs after this one: read the colours again next frame.
      requestAnimationFrame(() => this.colors.clear());
      const css = getComputedStyle(document.documentElement);
      this.fonts = {
        mono: `${css.getPropertyValue("--font-jetbrains-mono").trim() || "ui-monospace"}, ui-monospace, monospace`,
        sans: `${css.getPropertyValue("--font-geist-sans").trim() || "ui-sans-serif"}, ui-sans-serif, sans-serif`,
      };
    }
    if (prevView !== null && prevView !== props.view) {
      if (props.view === "orbit") this.cam.pitchT = 0.55;
      if (props.view === "circle") this.cam.pitchT = CIRCLE_PITCH;
      this.cam.pxT = 0;
      this.cam.pyT = 0;
    }
    if (props.graph !== this.lastGraph || props.view !== prevView) this.sync();
    this.lastView = props.view;
  }

  /** Adds new nodes at their parent's position, drops gone ones, and sets every target. */
  sync() {
    const { graph, view } = this.props;
    this.lastGraph = graph;
    const seen = new Set<string>();
    // What arrives with the first load appears in place; later nodes grow out of their parent and ease.
    const snap = performance.now() - this.born < 1500;
    for (const n of graph.nodes) {
      let live = this.nodes.get(n.id);
      if (!live) {
        const p = (n.parent && this.nodes.get(n.parent)) || this.nodes.get("orch");
        live = { ...n, x: p?.x ?? 0, y: p?.y ?? 0, z: p?.z ?? 0, a: 0, tx: 0, ty: 0, tz: 0, ta: 1, sx: 0, sy: 0, ss: 1, sz: 0, fa: 1, da: 1, vis: true, match: false };
        this.nodes.set(n.id, live);
      } else Object.assign(live, n);
      seen.add(n.id);
    }
    for (const k of [...this.nodes.keys()]) if (!seen.has(k)) this.nodes.delete(k);
    const targets = layout(graph, view);
    for (const [id, n] of this.nodes) {
      const t = targets.get(id) ?? [0, 0, 0];
      n.tx = t[0];
      n.ty = t[1];
      n.tz = t[2];
      n.ta = view === "graph" && n.type === "run" ? 0.55 : 1;
      if (snap) {
        n.x = n.tx;
        n.y = n.ty;
        n.z = n.tz;
        n.a = n.ta;
      }
    }
  }

  fit() {
    this.cam.zoomT = 1;
    this.cam.pxT = 0;
    this.cam.pyT = 0;
    this.flyTo = null;
  }

  fly(id: string) {
    this.flyTo = id;
    this.flyN = 50;
    const n = this.nodes.get(id);
    if (n && this.props.view === "orbit") {
      this.cam.yawT = Math.atan2(n.tx, n.tz) + Math.PI;
      while (this.cam.yawT - this.cam.yaw > Math.PI) this.cam.yawT -= Math.PI * 2;
      while (this.cam.yaw - this.cam.yawT > Math.PI) this.cam.yawT += Math.PI * 2;
      this.cam.pitchT = 0.2;
    }
    this.cam.zoomT = Math.max(this.cam.zoomT, 1.6);
  }

  hit(mx: number, my: number): string | null {
    let best: string | null = null;
    let bd = 1e9;
    let run: string | null = null;
    let rd = 1e9;
    for (const n of this.nodes.values()) {
      if (!n.vis || n.a * n.fa < 0.15) continue;
      const d = Math.hypot(n.sx - mx, n.sy - my);
      if (d >= HIT[n.type] * Math.max(0.7, n.ss)) continue;
      if (n.type === "run") {
        if (d < rd) {
          rd = d;
          run = n.id;
        }
      } else if (d < bd) {
        bd = d;
        best = n.id;
      }
    }
    return best ?? run;
  }

  /** A CSS colour (`var(--x)` or literal) as the canvas can use it. */
  c(css: string): string {
    let v = this.colors.get(css);
    if (v === undefined) {
      const m = /^var\((--[\w-]+)\)$/.exec(css);
      v = m ? getComputedStyle(document.documentElement).getPropertyValue(m[1]).trim() || "#888" : css;
      this.colors.set(css, v);
    }
    return v;
  }

  /** `text` cut with an ellipsis to `room` pixels in the current font. */
  clip(text: string, room: number): string {
    if (room === Infinity || this.ctx.measureText(text).width <= room) return text;
    let lo = 0;
    let hi = text.length;
    while (lo < hi) {
      const mid = (lo + hi + 1) >> 1;
      if (this.ctx.measureText(`${text.slice(0, mid)}…`).width <= room) lo = mid;
      else hi = mid - 1;
    }
    return `${text.slice(0, lo)}…`;
  }

  mono(size: number, weight = 500) {
    return `${weight} ${size}px ${this.fonts.mono}`;
  }

  draw(time: number) {
    const { ctx, W, H, cam } = this;
    const p = this.props;
    const view = p.view;
    const A = this.c("var(--accent)");
    const line = this.c("var(--text)");
    // Live activity (lib/activity.ts): when each worker appeared, task and session changes, new output lines.
    const M = activityMarks();
    const wall = Date.now();
    const still = !p.motion || this.reduce;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    for (const n of this.nodes.values()) {
      n.x += (n.tx - n.x) * 0.075;
      n.y += (n.ty - n.y) * 0.075;
      n.z += (n.tz - n.z) * 0.075;
      n.a += (n.ta - n.a) * 0.06;
    }
    if (view === "orbit") {
      if (p.motion && !this.dragging && !this.flyTo) cam.yawT += 0.0016;
    } else {
      cam.yawT = Math.round(cam.yaw / (Math.PI * 2)) * Math.PI * 2;
      cam.pitchT = view === "circle" ? CIRCLE_PITCH : 0;
    }
    cam.yaw += (cam.yawT - cam.yaw) * 0.08;
    cam.pitch += (cam.pitchT - cam.pitch) * 0.08;
    cam.zoom += (cam.zoomT - cam.zoom) * 0.12;
    const Rt = (view === "graph" ? Math.min(W / 3.4, H / 2.05) : Math.min(W, H) * 0.38) * cam.zoom;
    const first = this.frames++ === 0;
    cam.R = first ? Rt : cam.R + (Rt - cam.R) * 0.1;
    if (this.flyTo) {
      const f = this.nodes.get(this.flyTo);
      if (f && this.flyN-- > 0) {
        cam.pxT -= (f.sx - W / 2) * 0.12;
        cam.pyT -= (f.sy - H / 2) * 0.12;
      } else this.flyTo = null;
    }
    cam.px += (cam.pxT - cam.px) * 0.15;
    cam.py += (cam.pyT - cam.py) * 0.15;
    const oxT = view === "graph" ? 0.18 : 0;
    cam.ox = first ? oxT : cam.ox + (oxT - cam.ox) * 0.08;
    for (const v of Object.keys(this.ga) as View[]) this.ga[v] += ((v === view ? 1 : 0) - this.ga[v]) * 0.07;
    const R = cam.R;
    const cx = W / 2 + cam.px + cam.ox * R;
    const cy = H / 2 + cam.py - 12;
    const cyw = Math.cos(cam.yaw);
    const syw = Math.sin(cam.yaw);
    const cp = Math.cos(cam.pitch);
    const sp = Math.sin(cam.pitch);
    const F = 3.2;
    const P = (x: number, y: number, z: number): [number, number, number, number] => {
      const x1 = x * cyw - z * syw;
      const z1 = x * syw + z * cyw;
      const y1 = y * cp - z1 * sp;
      const z2 = y * sp + z1 * cp;
      const s = F / (F + z2);
      return [cx + x1 * s * R, cy + y1 * s * R, s, z2];
    };
    const poly = (pts: [number, number, number][], alpha: number, w = 1) => {
      ctx.beginPath();
      pts.forEach((pt, i) => {
        const q = P(pt[0], pt[1], pt[2]);
        if (i) ctx.lineTo(q[0], q[1]);
        else ctx.moveTo(q[0], q[1]);
      });
      ctx.globalAlpha = alpha;
      ctx.strokeStyle = line;
      ctx.lineWidth = w;
      ctx.stroke();
      ctx.globalAlpha = 1;
    };
    const ring = (r: number, y0: number) => Array.from({ length: 73 }, (_, i) => [Math.cos((i / 72) * Math.PI * 2) * r, y0, Math.sin((i / 72) * Math.PI * 2) * r] as [number, number, number]);
    const spacing = (px: number) => {
      if ("letterSpacing" in ctx) (ctx as CanvasRenderingContext2D & { letterSpacing: string }).letterSpacing = `${px}px`;
    };
    spacing(0);

    const go = this.ga.orbit + this.ga.circle;
    const gg = this.ga.graph;
    if (go > 0.01) {
      // One flat ring per layer, named where it passes nearest the viewer.
      const names: Record<OrbitLayer, string> = { group: p.all ? "PROJECTS" : "PROVIDERS", session: "SESSIONS", task: "TASKS", run: "RUNS", stage: "PIPELINE" };
      ctx.font = this.mono(8.5);
      spacing(1.5);
      ctx.textAlign = "left";
      for (const [layer, r] of Object.entries(ORBIT_RINGS) as [OrbitLayer, number][]) {
        poly(ring(r, 0), (layer === "stage" ? 0.2 : 0.12) * go, layer === "stage" ? 1.2 : 1);
        const t = -Math.PI / 2 - cam.yaw + 0.5;
        const q = P(Math.cos(t) * r, 0, Math.sin(t) * r);
        ctx.globalAlpha = 0.55 * go;
        ctx.fillStyle = this.c("var(--muted)");
        ctx.fillText(names[layer], q[0] + 4, q[1] - 4);
        ctx.globalAlpha = 1;
      }
      spacing(0);
    }
    // Column headers need room; on a phone the nodes speak for themselves.
    if (gg > 0.02 && W >= 600) {
      ctx.font = this.mono(9.5);
      spacing(2);
      ctx.textAlign = "center";
      ctx.fillStyle = this.c("var(--muted)");
      ctx.globalAlpha = 0.9 * gg;
      for (const [x, label] of graphColumns(p.all)) {
        const q = P(x, -1.04, 0);
        ctx.fillText(label, q[0], q[1]);
      }
      ctx.globalAlpha = 1;
      spacing(0);
    }

    const q = p.search.trim().toLowerCase();
    const hov = this.hover;
    const sel = p.selected;
    const focusId = hov ?? sel;
    const nbr = new Set<string>();
    if (focusId) {
      nbr.add(focusId);
      for (const l of p.graph.links) {
        if (l.a === focusId) nbr.add(l.b);
        if (l.b === focusId) nbr.add(l.a);
      }
      if (focusId.startsWith("s:")) for (const [k, n] of this.nodes) if (n.type === "run" && n.parent === focusId) nbr.add(k);
    }
    const list: Live[] = [];
    for (const n of this.nodes.values()) {
      n.vis = !p.hidden[n.type];
      const pr = P(n.x, n.y, n.z);
      n.sx = pr[0];
      n.sy = pr[1];
      n.ss = pr[2];
      n.sz = pr[3];
      let fa = 1;
      if (p.filter !== "all" && n.group && n.group !== p.filter) fa = 0.08;
      n.match = !!q && n.type !== "run" && n.label.toLowerCase().includes(q);
      if (q && !n.match) fa *= 0.15;
      if (hov && !nbr.has(n.id)) fa *= 0.22;
      n.fa = fa;
      n.da = Math.max(0.28, Math.min(1, 1 - n.sz * 0.5));
      if (n.vis) list.push(n);
    }

    const bez = gg > 0.5;
    /** The point at `t` (0 at `a`, 1 at `b`) of the link as drawn: a horizontal S-curve in the graph view. */
    const along = (a: Live, b: Live, t: number): [number, number] => {
      if (!bez) return [a.sx + (b.sx - a.sx) * t, a.sy + (b.sy - a.sy) * t];
      const mx = (a.sx + b.sx) / 2;
      const u = 1 - t;
      return [u * u * u * a.sx + 3 * u * u * t * mx + 3 * u * t * t * mx + t * t * t * b.sx, u * u * u * a.sy + 3 * u * u * t * a.sy + 3 * u * t * t * b.sy + t * t * t * b.sy];
    };
    /** A just-spawned session's links draw themselves towards it: from its task, and from its provider hub. */
    const SPAWN_LINK = 1100;
    const spawnAge = (id: string) => (id.startsWith("s:") ? wall - (M.spawn.get(id.slice(2)) ?? -1e12) : Infinity);
    for (const l of p.graph.links) {
      const a = this.nodes.get(l.a);
      const b = this.nodes.get(l.b);
      if (!a || !b || !a.vis || !b.vis) continue;
      const toA = l.kind === "work";
      const sa = toA ? spawnAge(l.a) : l.kind === "own" ? spawnAge(l.b) : Infinity;
      if (sa < SPAWN_LINK && !still) {
        const prog = 1 - (1 - sa / SPAWN_LINK) ** 3;
        const col = this.c((toA ? a : b).color);
        ctx.strokeStyle = col;
        ctx.globalAlpha = 0.85 * Math.min(a.fa, b.fa);
        ctx.lineWidth = 1.6;
        ctx.beginPath();
        const steps = 18;
        for (let i = 0; i <= steps; i++) {
          const f = (i / steps) * prog;
          const [x, y] = along(a, b, toA ? 1 - f : f);
          if (i) ctx.lineTo(x, y);
          else ctx.moveTo(x, y);
        }
        ctx.stroke();
        const [hx, hy] = along(a, b, toA ? 1 - prog : prog);
        ctx.fillStyle = col;
        ctx.shadowColor = col;
        ctx.shadowBlur = 14;
        ctx.beginPath();
        ctx.arc(hx, hy, 2.6, 0, 7);
        ctx.fill();
        ctx.shadowBlur = 0;
        continue;
      }
      const touch = !!focusId && (l.a === focusId || l.b === focusId);
      let al = LINK_ALPHA[l.kind] * Math.min(a.a, b.a) * Math.min(a.fa, b.fa) * Math.min(a.da, b.da);
      if (touch) al = Math.min(1, al * 2.4 + 0.25);
      if (al < 0.01) continue;
      ctx.strokeStyle = l.kind === "hub" ? A : l.kind === "dep" || l.kind === "infra" ? line : l.kind === "stage" ? this.c("var(--info)") : this.c(a.color);
      ctx.globalAlpha = al;
      ctx.lineWidth = touch ? 1.5 : 1;
      ctx.beginPath();
      ctx.moveTo(a.sx, a.sy);
      if (bez) {
        const mx = (a.sx + b.sx) / 2;
        ctx.bezierCurveTo(mx, a.sy, mx, b.sy, b.sx, b.sy);
      } else ctx.lineTo(b.sx, b.sy);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    if (p.motion) {
      for (const l of p.graph.links) {
        if (!l.active) continue;
        const a = this.nodes.get(l.a);
        const b = this.nodes.get(l.b);
        if (!a || !b || !a.vis || !b.vis) continue;
        const al = Math.min(a.fa, b.fa) * Math.min(a.a, b.a);
        if (al < 0.05) continue;
        const col = l.kind === "stage" ? this.c("var(--info)") : l.kind === "hub" ? A : this.c(a.color);
        for (let j = 0; j < 2; j++) {
          const t = (time * 0.00032 + hash(l.id, 1) + j * 0.5) % 1;
          const [x, y] = along(a, b, t);
          ctx.globalAlpha = 0.95 * al;
          ctx.fillStyle = col;
          ctx.shadowColor = col;
          ctx.shadowBlur = 8;
          ctx.beginPath();
          ctx.arc(x, y, 1.8, 0, 7);
          ctx.fill();
        }
      }
      ctx.shadowBlur = 0;
      ctx.globalAlpha = 1;
    }

    list.sort((a, b) => b.sz - a.sz);
    type Label = { n: Live; x: number; y: number; font: string; color: string; alpha: number; align: CanvasTextAlign; sub?: string; text?: string; room?: number };
    const labels: Label[] = [];
    const diamond = (x: number, y: number, r: number) => {
      ctx.beginPath();
      ctx.moveTo(x, y - r);
      ctx.lineTo(x + r, y);
      ctx.lineTo(x, y + r);
      ctx.lineTo(x - r, y);
      ctx.closePath();
    };
    const hex = (x: number, y: number, r: number) => {
      ctx.beginPath();
      for (let i = 0; i < 6; i++) {
        const an = Math.PI / 6 + (i * Math.PI) / 3;
        const px = x + Math.cos(an) * r;
        const py = y + Math.sin(an) * r;
        if (i) ctx.lineTo(px, py);
        else ctx.moveTo(px, py);
      }
      ctx.closePath();
    };
    const circle = (x: number, y: number, r: number) => {
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 7);
    };
    const bg = this.c("var(--inset)");
    /** A ring that grows from `r0` to `r1` and fades over `dur` ms; with less motion it stays put and only fades. */
    const burst = (shape: (x: number, y: number, r: number) => void, x: number, y: number, r0: number, r1: number, age: number, dur: number, col: string, alpha: number) => {
      const t = age / dur;
      if (t < 0 || t >= 1) return;
      const e = 1 - (1 - t) ** 3;
      ctx.globalAlpha = alpha * (1 - t);
      ctx.strokeStyle = col;
      ctx.lineWidth = 0.6 + 1.4 * (1 - t);
      shape(x, y, Math.max(0.5, still ? (r0 + r1) / 2 : r0 + (r1 - r0) * e));
      ctx.stroke();
    };
    for (const n of list) {
      const al = n.a * n.fa * n.da;
      if (al < 0.01) continue;
      const s = n.ss;
      let x = n.sx;
      const y = n.sy;
      const color = this.c(n.color);
      ctx.globalAlpha = 1;
      if (n.ref.kind === "run") {
        ctx.globalAlpha = 0.62 * al;
        ctx.fillStyle = color;
        circle(x, y, 1.5 * s);
        ctx.fill();
        ctx.globalAlpha = 1;
        continue;
      }
      if (n.ref.kind === "session") {
        const se = n.ref.session;
        const sa = wall - (M.spawn.get(se.name) ?? -1e12);
        // A new agent pops out of its hub: a quick overshoot, then two rings in its provider's colour.
        const r = 4.6 * s * (sa < 520 && !still ? 1 + 0.75 * Math.sin((Math.PI * sa) / 520) : 1);
        const tc = this.c(`var(${SESSION_TONE[se.state] ?? "--idle"})`);
        if (sa < 1800) {
          burst(circle, x, y, r, r + 36 * s, sa, 1500, color, 0.95 * al);
          burst(circle, x, y, r, r + 22 * s, sa - 300, 1200, color, 0.6 * al);
        }
        const sm = M.session.get(se.name);
        if (sm?.from && sm.to === se.state) burst(circle, x, y, r + 2, r + 16 * s, wall - sm.at, 1200, tc, 0.85 * al);
        // The heartbeat: each new output line sends a small blip.
        const lineAt = M.line.get(se.name);
        if (lineAt !== undefined) burst(circle, x, y, r + 1, r + 8 * s, wall - lineAt, 650, tc, 0.6 * al);
        if (se.state === "exited") {
          ctx.globalAlpha = 0.6 * al;
          ctx.strokeStyle = color;
          ctx.lineWidth = 1.2;
          circle(x, y, r);
          ctx.stroke();
        } else {
          ctx.shadowColor = color;
          ctx.shadowBlur = 12 * al;
          ctx.globalAlpha = al;
          ctx.fillStyle = color;
          circle(x, y, r);
          ctx.fill();
          ctx.shadowBlur = 0;
          if (se.state === "working" || se.state === "starting") {
            const pr = r + 3 + 2.5 * (0.5 + 0.5 * Math.sin(time * 0.004 + hash(n.id, 9) * 6));
            ctx.globalAlpha = 0.5 * al;
            ctx.strokeStyle = tc;
            ctx.lineWidth = 1;
            circle(x, y, pr);
            ctx.stroke();
          } else if (se.state !== "idle") {
            ctx.setLineDash([2, 2]);
            ctx.globalAlpha = 0.95 * al;
            ctx.strokeStyle = tc;
            ctx.lineWidth = 1.4;
            circle(x, y, r + 3.5);
            ctx.stroke();
            ctx.setLineDash([]);
          }
        }
        if (p.names || n.match || n.id === focusId) labels.push({ n, x: x + r + 6, y: y + 3.5, font: this.mono(9.5), color: this.c("var(--text)"), alpha: 0.8 * al, align: "left" });
      } else if (n.ref.kind === "task") {
        const t = n.ref.task;
        const tm = M.task.get(t.id);
        const ta = tm && tm.to === t.status ? wall - tm.at : Infinity;
        // Failed: a short shake and a red flash. Done: the diamond overshoots and settles inside a closing ring.
        if (t.status === "failed" && ta < 700 && !still) x += Math.sin(ta * 0.075) * 5 * (1 - ta / 700);
        const settle = t.status === "done" && ta < 900 && !still ? 0.6 * Math.exp(-ta / 220) * Math.cos(ta / 70) : 0;
        const r = 4.6 * s * (1 + settle);
        if (ta < 1800) {
          const tone = t.status === "running" ? color : this.c(t.status === "failed" ? "var(--bad)" : t.status === "done" ? "var(--ok)" : t.status === "review" ? "var(--warn)" : "var(--info)");
          if (t.status === "failed") {
            ctx.globalAlpha = 0.35 * al * Math.max(0, 1 - ta / 1200);
            ctx.fillStyle = tone;
            circle(x, y, r + 9 * s);
            ctx.fill();
            burst(diamond, x, y, r, r + 26 * s, ta, 1200, tone, 0.9 * al);
          } else if (t.status === "done") {
            burst(diamond, x, y, r + 20 * s, r + 2, ta, 900, tone, 0.95 * al);
            burst(circle, x, y, r, r + 30 * s, ta - 400, 1300, tone, 0.45 * al);
          } else {
            // A spawn (pending → running) ripples out of the task in its provider's colour; each phase in its tone.
            burst(diamond, x, y, r, r + 40 * s, ta, 1500, tone, 0.95 * al);
            burst(diamond, x, y, r, r + 24 * s, ta - 280, 1200, tone, 0.55 * al);
          }
        }
        let fill: string | null = null;
        let fillA = 1;
        let stroke: string | null = null;
        let strokeA = 1;
        // A task the intake gate holds waits for the owner: outlined in the accent.
        if (t.status === "pending") stroke = this.c(isHeld(t) ? "var(--accent)" : "var(--text-2)");
        else if (t.status === "running") fill = color;
        else if (t.status === "verifying" || t.status === "auditing" || t.status === "landing") fill = this.c("var(--info)");
        else if (t.status === "review") fill = this.c("var(--warn)");
        else if (t.status === "done") {
          fill = color;
          fillA = 0.5;
        } else if (t.status === "failed") fill = this.c("var(--bad)");
        else {
          stroke = this.c("var(--idle)");
          strokeA = 0.4;
        }
        diamond(x, y, r);
        if (fill) {
          ctx.globalAlpha = fillA * al;
          ctx.fillStyle = fill;
          ctx.fill();
        }
        if (stroke) {
          ctx.globalAlpha = strokeA * al;
          ctx.strokeStyle = stroke;
          ctx.lineWidth = 1.2;
          ctx.stroke();
        }
        if (t.status === "running" || t.status === "verifying" || t.status === "auditing" || t.status === "landing") {
          const pr = r + 2.5 + 2 * (0.5 + 0.5 * Math.sin(time * 0.005 + hash(n.id, 3) * 6));
          ctx.globalAlpha = 0.4 * al;
          ctx.strokeStyle = t.status === "running" ? color : this.c("var(--info)");
          ctx.lineWidth = 1;
          diamond(x, y, pr);
          ctx.stroke();
        }
        if ((p.names && gg > 0.5 && !p.all) || n.match || n.id === focusId) {
          // In the graph view a label stops at the next column, unless it is the one in focus.
          const room = gg > 0.5 && n.id !== focusId ? depthStep(p.graph) * R * s - r - 12 : Infinity;
          labels.push({ n, x: x + r + 6, y: y + 3.5, font: this.mono(9.5), color: this.c("var(--text)"), alpha: 0.85 * al, align: "left", room });
        }
      } else if (n.ref.kind === "group") {
        const g = n.ref.group;
        const r = 10 * s;
        const avail = g.available !== false;
        ctx.globalAlpha = 0.16 * al;
        ctx.fillStyle = color;
        circle(x, y, r);
        ctx.fill();
        if (!avail) ctx.setLineDash([3, 3]);
        ctx.globalAlpha = (avail ? 0.95 : 0.45) * al;
        ctx.strokeStyle = color;
        ctx.lineWidth = 1.4;
        circle(x, y, r);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = al;
        ctx.fillStyle = color;
        ctx.font = `600 ${(10 * s).toFixed(1)}px ${this.fonts.sans}`;
        ctx.textAlign = "center";
        ctx.fillText((g.label[0] ?? "?").toUpperCase(), x, y + 3.5 * s);
        labels.push({ n, x: x + r + 7, y: y - 1, font: `600 11px ${this.fonts.sans}`, color: this.c("var(--text)"), alpha: 0.95 * al, align: "left", sub: `${p.graph.count[g.key] ?? 0}${avail ? "" : " · missing"}` });
      } else if (n.ref.kind === "stage") {
        const st = n.ref.stage;
        const r = 9 * s;
        const act = p.graph.activeStages.has(n.id);
        const info = this.c("var(--info)");
        hex(x, y, r);
        ctx.globalAlpha = 0.95 * al;
        ctx.fillStyle = bg;
        ctx.fill();
        if (act) {
          ctx.shadowColor = info;
          ctx.shadowBlur = 12;
        }
        ctx.globalAlpha = (act ? 1 : 0.5) * al;
        ctx.strokeStyle = act ? info : line;
        ctx.lineWidth = 1.3;
        ctx.stroke();
        ctx.shadowBlur = 0;
        ctx.globalAlpha = (act ? 1 : 0.85) * al;
        ctx.fillStyle = act ? info : this.c("var(--text-2)");
        ctx.font = this.mono(+(7.5 * s).toFixed(1), 600);
        ctx.textAlign = "center";
        ctx.fillText(st.glyph, x, y + 2.6 * s);
        if (p.names || n.id === focusId) labels.push({ n, x, y: y + r + 12, font: this.mono(9), color: this.c("var(--muted)"), alpha: 0.85 * al, align: "center", text: st.label.split(" ·")[0] });
      } else if (n.ref.kind === "orch") {
        const r = 15 * s;
        ctx.shadowColor = A;
        ctx.shadowBlur = 22;
        hex(x, y, r);
        ctx.globalAlpha = al;
        ctx.fillStyle = bg;
        ctx.fill();
        ctx.strokeStyle = A;
        ctx.lineWidth = 2.2;
        ctx.stroke();
        ctx.shadowBlur = 0;
        hex(x, y, r * 0.42);
        ctx.globalAlpha = (p.running ? 0.85 : 0.3) * al;
        ctx.fillStyle = A;
        ctx.fill();
        ctx.font = this.mono(10, 600);
        spacing(1.5);
        const tw = ctx.measureText(n.label).width;
        ctx.globalAlpha = 0.85 * al;
        ctx.fillStyle = bg;
        ctx.fillRect(x - tw / 2 - 8, y + r + 8, tw + 16, 18);
        ctx.globalAlpha = 0.15 * al;
        ctx.strokeStyle = line;
        ctx.lineWidth = 1;
        ctx.strokeRect(x - tw / 2 - 8, y + r + 8, tw + 16, 18);
        ctx.globalAlpha = al;
        ctx.fillStyle = this.c("var(--text)");
        ctx.textAlign = "center";
        ctx.fillText(n.label, x, y + r + 21);
        spacing(0);
      }
      ctx.globalAlpha = 1;
      if (n.id === sel) {
        ctx.setLineDash([3, 3]);
        ctx.lineDashOffset = -time * 0.02;
        ctx.strokeStyle = A;
        ctx.lineWidth = 1.4;
        circle(x, y, (n.type === "orch" ? 24 : n.type === "group" ? 16 : 11) * s);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.lineDashOffset = 0;
      } else if (n.match) {
        ctx.globalAlpha = 0.85;
        ctx.strokeStyle = line;
        ctx.lineWidth = 1.2;
        circle(x, y, 10 * s);
        ctx.stroke();
        ctx.globalAlpha = 1;
      }
    }
    for (const L of labels) {
      ctx.font = L.font;
      ctx.textAlign = L.align;
      ctx.fillStyle = L.color;
      ctx.globalAlpha = Math.max(0, Math.min(1, L.alpha));
      ctx.fillText(this.clip(L.text ?? L.n.label, L.room ?? Infinity), L.x, L.y);
      if (L.sub != null) {
        ctx.font = this.mono(9.5);
        ctx.fillStyle = this.c(L.n.color);
        ctx.globalAlpha = Math.max(0, Math.min(1, 0.9 * L.n.a * L.n.fa));
        ctx.fillText(L.sub, L.x, L.y + 12);
      }
    }
    ctx.globalAlpha = 1;
    if (hov) {
      const n = this.nodes.get(hov);
      if (n && n.type === "run") {
        ctx.font = this.mono(10);
        const tw = ctx.measureText(n.label).width;
        ctx.globalAlpha = 0.9;
        ctx.fillStyle = bg;
        ctx.fillRect(n.sx + 10, n.sy - 22, tw + 14, 18);
        ctx.globalAlpha = 1;
        ctx.fillStyle = this.c("var(--text)");
        ctx.textAlign = "left";
        ctx.fillText(n.label, n.sx + 17, n.sy - 9);
      }
    }
  }
}

/** The canvas element plus its engine; props flow into the engine after every render. */
export function CommandCanvas(props: CanvasProps) {
  const ref = useRef<HTMLCanvasElement>(null);
  const engine = useRef<Engine | null>(null);
  const { controlsRef } = props;

  useEffect(() => {
    const e = new Engine(ref.current!, props);
    engine.current = e;
    controlsRef.current = { fit: () => e.fit(), fly: (id) => e.fly(id) };
    return () => {
      e.destroy();
      engine.current = null;
      controlsRef.current = null;
    };
    // The engine is created once; later props reach it through the effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    engine.current?.update(props);
  });

  return <canvas ref={ref} className="absolute inset-0 block size-full cursor-grab touch-none" />;
}
