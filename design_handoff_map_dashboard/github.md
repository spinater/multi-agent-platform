repo: spinater/multi-agent-platform
branch: main

## Last sync
date: 2026-10-13T15:57:40Z

### Updated in this project
- New Projects page: pick a workspace, or create one with a 5-step wizard that writes map.toml
- New Skills page: project, user and plugin skills, context files per provider, skill × task matrix
- New Decisions page: live orchestrator decision model (ready → pick → launch → watch → verify → audit → post_done → end decision, plus retry/fail)
- Shared top nav across all screens
- Command centre data is now per project (sessions, tasks, events saved separately for each workspace)

## Notes
- Reflex is not in the repo yet; Reflex.dc.html is designed from the spec the user provided.

## Screen map
| Screen | Repo files |
| --- | --- |
| Agent Command Centre.dc.html | backend/src/model.rs, frontend/components/dashboard.tsx, frontend/components/sessions.tsx, frontend/components/tasks.tsx, frontend/components/ui.tsx, frontend/app/globals.css, map.example.toml, README.md |
| Projects.dc.html | map.example.toml, backend/src/config.rs, README.md |
| Skills.dc.html | AGENTS.md, CLAUDE.md |
| Reflex.dc.html (+ ReflexFlow, ReflexSetup, ReflexPlayground, ReflexCalibrate, ReflexLog) | — (spec from chat) |
| Usage.dc.html | README.md (watcher /usage readers), backend/src/detect.rs (limit detection) |
| Decisions.dc.html | backend/src/orchestrator.rs, backend/src/detect.rs (end_decision), AGENTS.md |

## Sync history
- 2026-10-13T12:01:06Z · initial import; command centre built
