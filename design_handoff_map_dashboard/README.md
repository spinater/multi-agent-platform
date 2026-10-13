# Handoff: MAP dashboard (multi-agent platform)

## Overview
UI for `spinater/multi-agent-platform` (Rust backend + Next.js `frontend/`). MAP runs several coding agents (claude, codex, gemini, aider; Copilot appears on the Usage screens only) in tmux sessions against one workspace, routes tasks between them, verifies and audits results. These designs cover:

- **Projects**: list of workspaces and a 5-step "new project" wizard that writes `map.toml`
- **Command centre**: live view of the orchestrator, providers, sessions, tasks and runs, with orbit (3D) and graph views, and an all-projects mode
- **Skills**: skills/context files each agent can load per project, plus a skill × task matrix
- **Decisions**: the orchestrator's per-task decision model (from `backend/src/orchestrator.rs`), animated live
- **Reflex**: local decision engine that answers choice/score/yes-no questions from a corpus and falls back to an LLM (3 layout options)
- **Usage**: plan limits per provider account (5-hour / 7-day windows, daily requests, Copilot premium requests)
- **UsageMini**: a compact usage indicator in the top bar of every screen, with a popover and an optional pinned corner dock

## About the design files
The `.dc.html` files in this bundle are **design references built in HTML**: prototypes that show the intended look and behaviour. They are not production code. Recreate them in the existing `frontend/` app (Next.js App Router, React, TypeScript), following its patterns (`components/ui.tsx`, `lib/use-dashboard.ts`, generated types in `lib/generated/`). Wire the data to the real backend (`/api/*` + WebSocket `ServerMessage`) instead of the simulated data in the prototypes.

To view a prototype, open any `.dc.html` file in a browser with `support.js` in the same folder (it is the small runtime that renders them). Each file holds an HTML template plus a `class Component` whose `renderVals()` shows exactly how each value on screen is computed. Read it as a spec.

## Fidelity
**High-fidelity.** Colours, type, spacing, copy and interactions are final, except where noted below. All data is simulated: provider plans, quotas, model names, Copilot multipliers, Reflex corpus/cases, version numbers and CLI commands such as `map reflex install` are placeholders to replace with real values.

## Global shell (all screens)
- **Top bar**, sticky, min-height 44px, padding 8px 20px, `border-bottom: 1px solid rgba(255,255,255,.07)`, background `rgba(7,8,10,.97)`. Wraps on narrow screens.
  - Left: `⬡ MAP` logo (Geist 700 14px, letter-spacing .08em, ⬡ in accent), project switcher button (28px tall, 1px border rgba(255,255,255,.12), radius 7px, JetBrains Mono 500 11.5px, 6px accent dot, `▾`). The dropdown is 290px wide, background #0c0e13, radius 10px, header "SWITCH PROJECT", one row per project with dot + ✓ for the current one, then "All projects" and "+ New project".
  - Nav links: Projects, Command centre, Skills, Decisions, Reflex, Usage. Each is 28px tall, padding 0 10px, radius 6px, Geist 500 12px, colour #c5ccd6. Hover: bg rgba(255,255,255,.05), colour #e6e9ef. Active: bg rgba(255,122,26,.12), colour #ffb37a.
  - Right: `UsageMini` pill, then clock `DD.MM HH:MM:SS` (JetBrains Mono 500 10px, letter-spacing .12em, #7a8494).
- **Page header** pattern: kicker (JetBrains Mono 500 10px, letter-spacing .2em, uppercase, #7a8494), H1 (Geist 600 38px/1.05, letter-spacing −.02em), lead paragraph (Geist 400 14px/1.55, #a3acb9, max-width ~700px).
- **Main**: max-width 1440px, centred, padding 32px 24px 60px, vertical gap 22–26px.
- The current project is shared across screens (prototype: `localStorage['map.project']`). In the real app, use a route param or global store.

## Screens

### Projects (`Projects.dc.html`)
- **Purpose:** pick the active workspace or create a new one.
- **List:** one card per project with status dot, name, path, repo · branch, provider chips (provider colours), task counts by status, live session count, time of last activity, an "N need you" attention badge, and actions to open the command centre or skills.
- **Search** filters by name/path.
- **New project wizard** (modal, 5 steps with a step rail: Source · Identity · Providers · Checks · Review):
  1. **Where does the code live?** Local folder or git repo (tabs). Path/repo inputs, branch.
  2. **Name it.** Project name and tmux session prefix (`map-`).
  3. **Which agents can work here?** Each provider row shows found/not found, default model, sign-in state (opens the auth modal), order (drag/up-down), and max parallel sessions.
  4. **How is a task proven done?** Verify command(s), plus whether the audit step is on and which provider runs it.
  5. **Review map.toml.** Generated TOML with syntax colours. "Create" runs a simulated `map check` (✓ lines appear one by one).
- **Auth modal:** browser sign-in with a one-time code (JetBrains Mono 600 30px, #ffb37a) and URL, or an API key. Shows the credential storage path.

### Command centre (`Agent Command Centre.dc.html`)
- **Purpose:** watch and steer the orchestrator for one project, or for all projects.
- **Status bar** (replaces the plain top bar): core running/paused, watcher · 5s, ws · live, `sqlite · .map/map.db`, workspace path, UsageMini, clock.
- **Scope toggle** next to "command centre": `<project>` | `All projects`.
- **Left column:** orchestrator state (Running/Paused, 36px), Run/Pause button, active/max-parallel count, providers with sign-in state, task queue (+ New task), sessions (+ New), "Needs you" list (sessions waiting for input, needing auth, stalled or errored, and failed tasks).
- **Centre canvas:** two views.
  - **Orbit (3D):** drag to orbit, wheel to zoom, click a node for its card.
  - **Graph:** left→right columns ORCH · PROVIDERS · SESSIONS · TASKS (by dependency depth) · PIPELINE.
  - Node glyphs: ◇ task, ○ session, ⬡ stage, • run. Links: hub, own, work, dep; active links animate.
  - Controls: provider chips that isolate one provider, type toggles, name/motion toggles, search, full-screen.
- **Right column:** detail card for the selected node (kicker, title, status badge, key/value rows, log blocks, actions; live sessions get a terminal input that sends a prompt), and an event feed (time · kind · message · who).
- **All-projects mode:**
  - Defaults to the **graph** view.
  - The PROVIDERS column becomes PROJECTS, coloured per project.
  - Tasks, sessions and events from every project are merged, and IDs show as `<project> #<id>`.
  - It is read-only: the left column shows a per-project list (state, live sessions, running, done/total, "need you") with "Open →" buttons. Detail cards only offer "Open in <project> →".
- **Modals:**
  - New task: title, prompt, provider (optional), model chips, dependency chips.
  - New session: provider, model, name, initial prompt, supervisor toggle.
  - Auth: same as on the Projects screen.

### Skills (`Skills.dc.html`)
- **Purpose:** see what each agent can load in this workspace and where it comes from.
- **Contents:**
  - Context files per provider (CLAUDE.md, AGENTS.md, GEMINI.md…)
  - Filters by source (project / user / plugin) and provider, plus search
  - Skill cards (name, source, description, allowed tools, load count)
  - Detail panel with "Enable/Disable for this project", "Run as task" (queues a task in the command centre), and "Copy path"
  - **Skill × task matrix**

### Decisions (`Decisions.dc.html`)
- **Purpose:** show how each task moves through the orchestrator: ready → pick provider → launch → watch → verify → audit → post_done → end decision, plus the retry/fail branches.
- Animated canvas with task tokens moving between nodes. Hovering or clicking a node shows its rule and the tasks currently in it. Each task has a trace of past decisions.
- Uses the same task data as the command centre when it exists for the project.

### Reflex (`Reflex.dc.html` + `ReflexFlow`, `ReflexSetup`, `ReflexPlayground`, `ReflexCalibrate`, `ReflexLog`)
Reflex is not in the repo yet. This screen was designed from a written spec.
- **Own theme tokens** (dark + light), Geist Mono throughout:
  - dark: `--bg #0b0c0e`, `--panel #111317`, `--panel2 #16191e`, `--line rgba(255,255,255,.09)`, `--line2 rgba(255,255,255,.17)`, `--text #e7e9ec`, `--muted #98a0ab`, `--faint #68707c`, `--accent #f08a3c`, `--accent-ink #0b0c0e`, `--reflex #4ade80`, `--llm #f2c14e`, `--abstain #f26d6d`
  - light: `--bg #f4f4f1`, `--panel #ffffff`, `--panel2 #f0f0ec`, `--line rgba(0,0,0,.1)`, `--line2 rgba(0,0,0,.2)`, `--text #15171a`, `--muted #555c66`, `--faint #80868f`, `--accent #c4621a`, `--accent-ink #fff`, `--reflex #15803d`, `--llm #a16207`, `--abstain #c53030`
- **Lane colours are fixed everywhere:** reflex green, llm yellow/orange, abstain red.
- **Confidence** ranges from about 1e-5 to 1e-3. Always show it on a **log scale** with ticks 1e-5 · 3e-5 · 1e-4 · 3e-4 · 1e-3, formatted like `2.1e-4`. Never as a percentage. Position on the axis: `(log10(c) + 5) / 2`.
- **Sections:**
  1. **How it works.** Question → Reflex (local, no model; corpus size) → gate `confidence ≥ <threshold>?` → three lane rows (yes → reflex, no → llm, nobody → abstain), each with % share, a bar and average latency. The footnote gives the real 24 h numbers.
  2. **Set up.** A 5-step checklist that ticks itself from real state:
     - Install
     - Write the corpus: `docs/reflex/<domain>/*.md`, 1–8 domains, English runbook-style notes
     - Define questions
     - Calibrate: at least 16 "message | correct answer" cases
     - Try the playground

     The current step is highlighted in accent and has a CTA button. A segmented progress bar sits on top.
  3. **Playground.** Pick a question (choice / yes-no / score) and paste input. The result shows lane badge, answer, ms, reason, the confidence dot against the threshold on the log axis, and per-option scores. Two feedback buttons, "✓ Correct" and "✕ Wrong", add the input as a calibration case.
  4. **Calibrate.** A scatter of cases on the log axis (correct = filled dots in the top band, wrong = red-ring dots in the bottom band). The threshold line can be dragged. Tiles update live: LLM calls saved %, Reflex wrong %, still to LLM %, threshold. Actions: "Suggest (≤ 5% wrong)" (picks the lowest threshold with at most 5% wrong) and "Save threshold". Before the first calibration there is one action, "Run calibration", which needs 16 cases. Cases can be pasted as `message | answer` lines.
  5. **Decision log + summary.** Tiles for LLM calls saved %, count and average ms per lane, and feedback ✓/✕. A 24-hour stacked bar chart by lane. A filterable log (time, lane, answer · question, input, confidence, ms, feedback).
- **States** (each shows a banner with a CTA, and the mode badge top-right):
  - not installed
  - no corpus
  - more than 8 domains (paused, offers to archive the smallest)
  - no questions
  - not calibrated (shadow mode: Reflex scores, but every question goes to the LLM)
  - Thai input: detected by `/[\u0E00-\u0E7F]/`, routed to the LLM with an explanation
- **Layout options** (prototype switcher; pick one to build):
  - **A · Guided:** sections 01–05 stacked, with Playground and Calibrate side by side
  - **B · Workbench:** sticky left rail (lane mini-stats + setup) and tabs on the right
  - **C · Console:** pipeline across the top, then two columns (Ask + Setup | Decisions + Calibrate)
- **Style rules:** 1px borders, no shadows, uppercase spaced labels. Must work on mobile (all grids use `auto-fit/minmax` and wrap).

### Usage (`Usage.dc.html`)
- **Purpose:** plan limits for each agent account on this machine. They are shared by all projects.
- **Summary strip:** one chip per provider with its tightest window % and name.
- **Provider cards** (grid `repeat(auto-fit, minmax(320px, 1fr))`, gap 14px, radius 14px):
  - Header: dot, name, plan · "read via <command>", status badge (ok / high / skipped / limited)
  - One meter per window:
    - label and value (% or `used / cap`)
    - 8px bar with the used fill in the provider colour (yellow at or above the skip line, red at 100%)
    - a striped projection segment (estimated use by reset at the last hour's pace)
    - a white 1px skip line
    - kind · "resets in Xh YYm · HH:MM"
    - pace note (e.g. "at this pace: skip line at 16:40, limit at reset")
  - Token row: input / output / cache for today
  - Copilot only: model breakdown (model, × multiplier, requests)
  - "By project · this window" stacked bar with legend
- **Windows per provider** (confirm against each CLI's real output):
  - Claude Code: 5-hour session, 7-day all models, 7-day Opus (`/usage`)
  - Codex: 5-hour, weekly (`/status`)
  - Gemini CLI: requests per day, requests per minute (`/stats`)
  - GitHub Copilot: premium requests per month, with model multipliers
- **"↻ Read now"** re-reads all providers and shows "read Ns ago".
- **Skip line:** the orchestrator stops assigning new tasks to a provider once any of its windows passes N% (default 90, a setting). This should map to the backend's provider cooldown in `config.rs` / `detect.rs`.

### UsageMini (`UsageMini.dc.html`, in the top bar of every screen)
- **Pill** (28px):
  - Per provider, two 3×14px vertical gauges: the short window and the long window.
  - Text: the tightest window, e.g. "Claude 7d Opus 88%". If any provider is over the skip line, the text becomes "<names> is/are over 90%" in #F2C14E and the border turns yellow.
- **Popover** (360px, `position: fixed`):
  - Anchored under the pill. It opens toward whichever side has room and is clamped to the viewport.
  - Rows per provider with mini meters, value and countdown.
  - "Pin to corner" toggle and "Open usage →".
- **Pinned dock:**
  - Fixed bottom-right, 300px.
  - One line per provider: dot, name, tightest bar, %, countdown.
  - Collapse (–/+) and unpin (×).
  - Pin and collapse state persist across pages.

## Interactions & behaviour (shared)
- Countdowns and the clock tick every second. Data refreshes from the WebSocket; the watcher polls every 5 s.
- Modals close on backdrop click. Dropdowns close on outside click (full-screen transparent layer).
- Toasts appear bottom-centre for success/warn/error.
- Every layout wraps; there are no fixed widths on text boxes.

## State the real app needs
- `currentProject`, `scope: 'project' | 'all'`, `view: 'orbit' | 'graph'`
- Per project: sessions, tasks, events (already in the backend `Overview` / `ServerMessage`)
- Provider auth state, per-provider usage windows `{label, kind, used, cap?, resetAt, ratePerHour}` and token totals
- Reflex: `installed`, `domains[]`, `questions[]`, `cases[] {text, ok, conf}`, `threshold`, `log[] {at, question, input, answer, conf, lane, ms, feedback}`
- UI-only state: `usage.pinned`, `usage.dockOpen`, selected node, filters

## Design tokens (main shell)
- Background `#07080a`, with radial `#10131b → #07080a` at the top. Surfaces: `rgba(255,255,255,.02)`, `#05060a` (inset), popover/menu `#0c0e13`.
- Text: `#e6e9ef` primary, `#d6dde6`, `#c5ccd6` nav, `#a3acb9` secondary, `#7a8494` muted.
- Borders: `rgba(255,255,255,.06/.07/.08/.12/.16)`.
- Accent `#FF7A1A`; accent text `#ffb37a`; links `#ff9a4d` (hover `#ffbd8a`); accent tint `rgba(255,122,26,.12)`.
- Status: ok `#4ade80` / `#7ee2a8`, warn `#F2C14E` / `#fbbf24`, error `#F26D6D` / `#f87171`, info `#38bdf8`.
- Providers: claude `#F2A65A`, codex `#5EA8F2`, gemini `#B48CF2`, aider / copilot `#4FD1B5`.
- Project palette (all-projects mode): `#F2A65A #5EA8F2 #B48CF2 #4FD1B5 #F27A9B #C9D45C #E6E9EF #7DD3FC`.
- Type:
  - **Geist** (UI): 38/600 H1, 20/600 H2, 16/600 card title, 12.5–14 body
  - **JetBrains Mono** (data/labels): 10–11px labels, uppercase with .1–.2em tracking, 15–20px figures
  - Reflex uses **Geist Mono** only
- Radius: 4 (badges), 5–7 (buttons/inputs), 8–10 (rows, small cards), 12–14 (cards/modals).
- Spacing: 4, 6, 8, 10, 12, 14, 16, 18, 20, 24, 32.
- Shadows: only on floating layers (menus/popovers `0 20px 50px rgba(0,0,0,.5–.6)`, dock `0 12px 36px rgba(0,0,0,.45)`). None on cards.

## Assets
No images. Glyphs are Unicode (⬡ ◇ ○ • ▾ ✓ ✕ ↻ ⇔). Fonts come from Google Fonts: Geist, Geist Mono, JetBrains Mono.

## Files
- `Projects.dc.html`, `Agent Command Centre.dc.html`, `Skills.dc.html`, `Decisions.dc.html`, `Usage.dc.html`, `UsageMini.dc.html`
- `Reflex.dc.html` with children `ReflexFlow.dc.html`, `ReflexSetup.dc.html`, `ReflexPlayground.dc.html`, `ReflexCalibrate.dc.html`, `ReflexLog.dc.html`
- `support.js`: prototype runtime only, needed to open the files in a browser
- `github.md`: source repo and which repo files each screen was built from
