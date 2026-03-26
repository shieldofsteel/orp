# ORP Development Harness — GAN-Inspired 3-Agent Architecture

_Based on Anthropic's harness design for long-running application development._
_Reference: https://www.anthropic.com/engineering/harness-design-long-running-apps_

## Architecture

```
User Prompt (1-4 sentences)
        │
        ▼
┌─────────────────┐
│    PLANNER       │  Expands prompt → full feature spec
│    (Opus 4.6)    │  Ambitious scope, high-level design
│                  │  Outputs: SPRINT_PLAN.md
└────────┬────────┘
         │
         ▼
┌─────────────────┐     ┌─────────────────┐
│   GENERATOR      │◄───│   EVALUATOR      │
│   (Opus 4.6)     │    │   (Opus 4.6)     │
│                  │───►│                  │
│ Builds one       │    │ Tests live app   │
│ feature at a     │    │ via Playwright   │
│ time. Commits.   │    │ Scores against   │
│                  │    │ criteria. Files  │
│ Reads evaluator  │    │ bugs. Pass/Fail. │
│ feedback, fixes  │    │                  │
│ or pivots.       │    │ Skeptical by     │
│                  │    │ default. Hard    │
│                  │    │ thresholds.      │
└─────────────────┘     └─────────────────┘
         │
    Loop until all features pass QA
         │
         ▼
    Push to GitHub
```

## The Three Agents

### 1. PLANNER
- **Input:** 1-4 sentence user prompt
- **Output:** `SPRINT_PLAN.md` — full product spec with features, acceptance criteria
- **Rules:**
  - Be ambitious about scope
  - Focus on product context and high-level design
  - Do NOT specify granular implementation details (let generator figure the path)
  - Include design language direction (dark military ops aesthetic, straight corners, monospace)
  - Identify opportunities for AI-powered features

### 2. GENERATOR (Droid — Claude Opus 4.6)
- **Input:** SPRINT_PLAN.md + evaluator feedback (if iteration > 1)
- **Output:** Working code, committed to git
- **Rules:**
  - Work on ONE feature at a time
  - Build, then self-evaluate before handing to QA
  - After evaluator feedback: refine current direction if scores trending well, pivot if not
  - `cargo test` + `cargo clippy` must pass before handoff
  - `npm run build` must pass for frontend changes
  - Commit after each feature with conventional commits
- **Command:**
  ```bash
  cd /Users/deepred/orp && droid exec \
    --auto high \
    --model "custom:Claude-Opus-4.6-(OAuth)-0" \
    --reasoning-effort high \
    -f .factory/task.md
  ```

### 3. EVALUATOR (Sub-agent with Browser)
- **Input:** Running application at localhost:9091
- **Tools:** Browser (Playwright), exec (curl, cargo test)
- **Output:** `EVAL_REPORT.md` — scores + bugs + critique
- **Rules:**
  - Actually navigate the live application via browser
  - Screenshot and study each page/feature
  - Test like a user, not like a developer
  - Be SKEPTICAL by default — assume things are broken until proven working
  - Grade against criteria (see below)
  - Hard thresholds — if any criterion fails, sprint fails

## Evaluation Criteria (ORP-Specific)

### 1. Functionality (threshold: 7/10)
- Can users perform the core action? (view entities on map, run queries, see alerts)
- Do all buttons/controls actually work when clicked?
- Are API responses correct and complete?
- Do WebSocket updates flow in real-time?
- Are there console errors?

### 2. Visual Quality (threshold: 6/10)
- Does it look like a military ops console, not a generic dashboard?
- Dark theme consistent? Straight corners? Monospace where appropriate?
- No "AI slop" patterns (purple gradients, card-in-card, gray text on colored bg)
- Typography hierarchy clear? Spacing consistent?
- Responsive — panels don't overflow or collapse?

### 3. Data Integrity (threshold: 8/10)
- Do entities appear with correct coordinates?
- Do entity counts match API responses?
- Are relationships and events loading?
- Is the pipeline flowing (connector → storage → API → UI)?
- No stale data, no phantom entities?

### 4. Completeness (threshold: 7/10)
- Does the feature match its spec/acceptance criteria?
- Are edge cases handled (empty states, errors, loading)?
- Are there stubs or TODO comments left?
- Does it integrate with existing features?

### 5. Performance (threshold: 6/10)
- Page load < 3 seconds?
- Map interaction smooth?
- Query results < 2 seconds?
- No memory leaks visible?

## Workflow

### Starting a Sprint

1. **Sentinel (me) writes the task prompt** (1-4 sentences)
2. **Planner sub-agent** expands → `SPRINT_PLAN.md`
3. **Generator droid** builds the features
4. **Start the server:** `ORP_DEV_MODE=true ./target/release/orp start --dev --port 9091`
5. **Evaluator sub-agent** tests live app via browser, writes `EVAL_REPORT.md`
6. **If PASS:** commit, push, next sprint
7. **If FAIL:** generator gets feedback, iterates (max 3 iterations per feature)

### Communication via Files

```
.factory/
├── HARNESS.md          — This file (architecture docs)
├── SPRINT_PLAN.md      — Current sprint spec (planner output)
├── task.md             — Current generator task (< 2KB)
├── EVAL_REPORT.md      — Latest evaluator report
├── EVAL_HISTORY.md     — All past evaluations
└── HANDOFF.md          — Context for next session (if context reset needed)
```

### Context Resets
- Opus 4.6 handles long sessions well — use compaction by default
- If session > 2 hours or agent shows confusion: context reset
- Write `HANDOFF.md` with: what's done, what's next, current bugs, file locations
- Fresh agent reads HANDOFF.md + SPRINT_PLAN.md to continue

## Anti-Patterns (from Anthropic's findings)

1. **Don't let the generator self-evaluate** — it will praise its own work
2. **Don't skip the evaluator on "simple" changes** — bugs hide in simple changes
3. **Don't over-specify implementation** — constrain deliverables, not paths
4. **Don't run evaluator on code alone** — it must interact with the LIVE app
5. **Don't ignore the evaluator's failures** — if it fails, the generator fixes it
6. **Don't compaction-only for long tasks** — context resets give a clean slate

## ORP Design Language (for evaluator criteria)

- **Aesthetic:** Military operations console. Dark backgrounds (#0a0e17, #0d1117). 
- **Typography:** JetBrains Mono / Fira Code for data. System sans-serif for UI.
- **Corners:** All straight. Zero rounded corners. `rounded-none` everywhere.
- **Colors:** Green=healthy/slow, Blue=normal, Orange=fast/warning, Red=critical/selected
- **Mood:** Serious, precise, professional. Not playful. Not generic.
- **Reference:** Palantir Gotham, Bloomberg Terminal, military COP systems
- **Anti-patterns:** No purple gradients, no card-in-card, no glassmorphism, no Inter font

## Quick Start

```bash
# 1. Planner
openclaw sessions_spawn --task "Plan the next ORP feature: [description]" --label planner

# 2. Generator  
cd /Users/deepred/orp && droid exec --auto high \
  --model "custom:Claude-Opus-4.6-(OAuth)-0" \
  --reasoning-effort high -f .factory/task.md

# 3. Start server for evaluation
ORP_DEV_MODE=true ./target/release/orp start --dev --port 9091

# 4. Evaluator
openclaw sessions_spawn --task "Evaluate ORP at localhost:9091 against SPRINT_PLAN.md" --label evaluator
```
