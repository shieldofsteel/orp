# GAN-Inspired Development Harness

_A skill for building production-quality applications using Anthropic's 3-agent architecture._
_Sources: Anthropic Engineering Blog — three posts by Prithvi Rajasekaran et al. (2025–2026):_
- _[Harness design for long-running application development](https://www.anthropic.com/engineering/harness-design-long-running-apps)_
- _[Effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)_
- _[Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)_

---

## When to Use This Skill

Use when:
- Building a new application or major feature from scratch
- Working on complex multi-session development tasks
- Quality matters more than speed
- The task involves both backend and frontend work
- You need to verify that features actually work (not just compile)

**Do NOT use for:** quick fixes, config changes, documentation-only tasks, or trivial feature additions the model handles reliably solo.

---

## Architecture: Planner → Generator → Evaluator

```
User Prompt (1-4 sentences)
        │
        ▼
   ┌─────────┐
   │ PLANNER  │  Expands prompt → full spec + design language
   └────┬────┘
        │  writes SPEC.md + feature_list.json
        ▼
   ┌─────────┐     ┌───────────┐
   │GENERATOR│◄───►│ EVALUATOR │  Tests LIVE app via browser automation
   └─────────┘     └───────────┘
      │  negotiates SPRINT_CONTRACT.md before building
      │
   Loop (refine or pivot) until all criteria pass thresholds
        │
        ▼
   Commit + Push
```

### Agent 1: PLANNER

**Role:** Takes a brief user prompt (1–4 sentences) and expands it into a comprehensive product spec.

**Key behaviors:**
- Be **ambitious about scope** — don't under-spec
- Focus on product context and **high-level design**, NOT granular implementation details (errors in upfront implementation detail cascade into downstream code)
- Include a design language direction (can read/reference the frontend-design skill)
- Actively look for opportunities to weave **AI-powered features** into the spec
- Output a structured spec (`SPEC.md`) + a `feature_list.json` file
- The feature list drives all subsequent work; it should be comprehensive (the claude.ai clone example had 200+ features)

**feature_list.json format — use JSON not Markdown** (models are less likely to inappropriately overwrite JSON):
```json
[
  {
    "category": "functional",
    "description": "New chat button creates a fresh conversation",
    "steps": [
      "Navigate to main interface",
      "Click the 'New Chat' button",
      "Verify a new conversation is created",
      "Check that chat area shows welcome state",
      "Verify conversation appears in sidebar"
    ],
    "passes": false
  }
]
```
All features start with `"passes": false`. **Never remove or edit features — only flip `passes` to `true`.**

**OpenClaw implementation:**
```javascript
// sessions_spawn to run planner
await sessions_spawn({
  task: `You are a product architect. Read this prompt and expand it into a full product spec.
  
USER PROMPT: "${userPrompt}"

Deliverables:
1. SPEC.md — ambitious feature list, design direction, high-level technical stack. Be ambitious about scope.
   Focus on WHAT to build, not HOW. Include design language (colors, typography, vibe).
   Look for opportunities to weave AI features into the product.
2. feature_list.json — every user-testable feature as JSON with description, steps[], passes:false.
   Use 50–200+ items. More is better.
3. init.sh — script to start the dev server (so future agents don't have to figure it out)

IMPORTANT: Do NOT specify granular implementation details in the spec. 
Specify deliverables; let the generator figure out the path.`,
  runtime: "subagent"
});
```

---

### Agent 2: GENERATOR

**Role:** Builds features one at a time from the spec, leaving the codebase in a clean state after each session.

**Key behaviors:**
- Work on **ONE feature at a time** (critical — prevents one-shotting and half-implemented states)
- Start every session by reading git log + progress files + feature_list.json + running init.sh
- Run a basic end-to-end smoke test before touching any new code — catch pre-existing breakage first
- Self-evaluate before handoff to evaluator (though don't rely on self-eval alone)
- After evaluator feedback: **refine** if scores trending well, **pivot to different approach** if not working
- Commit after each feature with descriptive git messages
- Leave environment in "clean state" (mergeable code: no bugs, orderly, well-documented)
- Use `git revert` to recover from bad changes — this is why git commits matter

**Session startup sequence (from Anthropic research):**
```
1. run pwd  — confirm working directory
2. read claude-progress.txt (or progress notes)
3. read feature_list.json — find highest-priority unfinished feature
4. git log --oneline -20 — see what was recently worked on
5. read init.sh — understand how to run the app
6. run init.sh — start the dev server
7. run basic smoke test (e.g., Puppeteer: open app, send test action, verify response)
8. fix any pre-existing bugs found in smoke test
9. begin implementing the next feature
10. end session: git commit + update claude-progress.txt
```

**Context management during long sessions:**
- If context window is filling up: write `HANDOFF.md` with current state, then do a **context reset** (fresh agent session reads HANDOFF.md)
- This differs from compaction — a reset provides a clean slate vs. compaction which preserves continuity but can still cause context anxiety
- Context anxiety: some models (notably Claude Sonnet 4.5) begin wrapping up work prematurely as they approach their perceived context limit. Resets cure this; compaction alone does not.
- Claude Opus 4.6 largely removed context anxiety, enabling continuous single sessions with automatic compaction.

**OpenClaw implementation (using droid):**
```bash
# Write task to file (keep under 2KB)
cat > .factory/build-task.md << 'EOF'
Read SPEC.md and feature_list.json. 
Run init.sh to start the dev server.
Run a basic smoke test to verify the app is working.
Pick the highest-priority feature with passes:false from feature_list.json.
Negotiate a sprint contract with the QA agent (write proposed contract to SPRINT_CONTRACT.md).
Implement the feature completely. Commit with descriptive message.
Self-evaluate before handing to QA.
Update claude-progress.txt with what was done.
EOF

droid exec \
  --auto high \
  --model "custom:Claude-Opus-4.6-(OAuth)-0" \
  --reasoning-effort high \
  -f .factory/build-task.md
```

---

### Agent 3: EVALUATOR

**Role:** Tests the LIVE running application like a real, skeptical user. Grades against agreed sprint contract.

**Key behaviors:**
- Navigate the application via browser automation (Playwright/Puppeteer MCP)
- **Actually click buttons, submit forms, check data renders correctly** — do not just read code
- Screenshot and carefully study each page before scoring
- Be **deeply skeptical** — assume broken until proven working
- Score each criterion with specific evidence
- If any criterion fails its threshold: sprint **FAILS** with specific bug reports including file + line references
- Write evaluator output to `EVAL_REPORT.md`

**Critical tuning note:** Out of the box, Claude is a poor QA agent. It will identify legitimate issues then talk itself into deciding they aren't a big deal. It tests superficially rather than probing edge cases. You MUST tune the evaluator by:
1. Reading evaluator logs
2. Finding where its judgment diverged from yours
3. Updating the evaluator prompt to address those specific patterns
4. Adding few-shot examples with detailed score breakdowns to calibrate it to your standards

**Calibration with few-shot examples:**
```
Here is an example of a well-graded evaluation:

CRITERION: Rectangle fill tool allows click-drag to fill rectangular area
FINDING: FAIL — Tool only places tiles at drag start/end points instead of filling the region.
fillRectangle function exists at src/editor/tools.ts:142 but isn't triggered properly on mouseUp.
SCORE: 2/10

CRITERION: User can reorder animation frames via API
FINDING: FAIL — PUT /frames/reorder route is defined AFTER /{frame_id} routes. FastAPI matches
'reorder' as a frame_id integer and returns 422: "unable to parse string as an integer."
SCORE: 1/10
```

**OpenClaw implementation:**
```javascript
await sessions_spawn({
  task: `You are a skeptical QA engineer. The application is running at http://localhost:3000.

Read SPRINT_CONTRACT.md to understand what was supposed to be built this sprint.
Read SPEC.md for full product context.

Use the browser tool to navigate and test the live application. 
Navigate the app as a real user would. Click buttons. Submit forms. 
Check that data persists. Verify real-time updates. Try edge cases.
Screenshot pages before scoring.

DO NOT trust the code. Test the running application.
DO NOT talk yourself into approving something that doesn't work.
If something seems broken, verify it is broken, then report it precisely.

Score each criterion (1-10) with specific evidence:
1. Functionality (threshold ≥7) — Do all features from the sprint contract work end-to-end?
2. Visual Design (threshold ≥6) — Coherent aesthetic? No AI slop patterns?
3. Data Integrity (threshold ≥8) — Correct data? No cross-tenant leaks? Persistent?
4. Completeness (threshold ≥7) — Matches sprint contract? Edge cases handled?
5. Code Quality (threshold ≥6) — No stubs? No half-implemented functions?

If ANY criterion is below threshold: overall result is FAIL.
Report specific bugs with file paths and line numbers where possible.

Write results to EVAL_REPORT.md in format:
OVERALL: PASS|FAIL
[criterion]: [score]/10 — [evidence]
BUGS: [specific issue at file:line]`,
  runtime: "subagent"
});
```

---

## Sprint Contract

Before building, generator and evaluator negotiate what "done" looks like for each chunk of work. This bridges the gap between high-level spec and testable implementation.

**Process:**
1. Generator writes proposed `SPRINT_CONTRACT.md`: what it will build, specific behaviors it will implement, how each behavior can be verified
2. Evaluator reviews and either approves or requests changes
3. They iterate until both agree
4. Generator builds against the contract
5. Evaluator grades against the contract

**Communication is via files** — one agent writes, another reads. No direct agent-to-agent conversation.

**Example sprint contract (Sprint 3 from Anthropic's research had 27 criteria):**
```markdown
# Sprint 3 Contract: Level Editor

## Features to implement
- Rectangle fill tool: click-drag fills rectangular area with selected tile
- Entity spawn points: user can place, select, and delete entity spawn points
- Animation frames: user can reorder frames via drag or API

## Test criteria
- [ ] Click-drag in tile layer fills all tiles in the dragged rectangle
- [ ] Clicking entity layer places a spawn point
- [ ] Selected entity spawn points highlighted with visual indicator
- [ ] Delete key removes selected spawn point
- [ ] PUT /frames/reorder updates frame order persistently
```

---

## Context Engineering (Critical for Long Tasks)

Context is a **finite resource with diminishing marginal returns**. Every token depletes the model's "attention budget." This is not a soft concern — studies show "context rot": as context grows, recall accuracy decreases across all models.

### System Prompt Design

The right "altitude" is a Goldilocks zone:
- **Too specific:** Brittle if-else logic hardcoded into prompts → fragile, high maintenance
- **Too vague:** Falsely assumes shared context → agent goes off-rails

**Best practices:**
- Use clear section headers: `<background_information>`, `<instructions>`, `## Tool guidance`, `## Output description`
- XML tags or Markdown headers to delineate sections
- Minimal set of information that fully outlines expected behavior (minimal ≠ short; give enough to ensure adherence)
- Start with minimal prompt, test with best model, add instructions based on failure modes found

### Tool Design Principles

Tools define the contract between agents and their action space. Design them to be:
- **Token efficient** — return only what's needed, not bloated responses
- **Self-contained and unambiguous** — if a human engineer can't tell which tool to use in a given situation, the agent can't either
- **Minimal overlap** — avoid two tools that do similar things
- **Robust to error** — handle failures gracefully
- **Descriptive parameters** — parameter names and descriptions should be unambiguous

Bloated tool sets are a common failure mode. Curate a minimal viable set.

### Few-Shot Examples

Provide diverse, canonical examples rather than an exhaustive list of edge cases. For LLMs, examples are "pictures worth a thousand words." A small set of high-quality examples beats a long list of rules.

### Just-in-Time Context Retrieval

Rather than loading all data upfront, agents should maintain **lightweight identifiers** (file paths, queries, URLs) and load data dynamically:
- Keep references, not the data itself
- Use tools like `glob`, `grep`, `head`, `tail` to navigate large codebases without loading everything
- Claude Code uses CLAUDE.md files (loaded upfront) + grep/glob for just-in-time file access
- This is the "hybrid strategy": some data upfront for speed, autonomous exploration at runtime

### Context Management Strategies for Long Tasks

**Three techniques (choose based on task type):**

#### 1. Compaction
- Summarize conversation nearing context limit; reinitiate with summary
- Preserve: architectural decisions, unresolved bugs, implementation details
- Discard: redundant tool outputs, repeated messages
- Lightest-touch compaction: tool result clearing (safe, minimal info loss)
- Best for: tasks requiring extensive back-and-forth
- Limitation: doesn't cure context anxiety (model may still feel it's near the limit)

**Claude Code compaction approach:** Pass message history to model; model preserves architectural decisions, unresolved bugs, implementation details; discards redundant tool outputs. Agent continues with compressed context + 5 most recently accessed files.

#### 2. Context Reset (Structured Handoff)
- Completely fresh agent session with a handoff artifact
- Handoff artifact must contain: what's done, current bugs, what's next, environment state
- Cures context anxiety entirely (clean slate)
- Higher cost: more tokens per session start, orchestration complexity, latency
- Best for: tasks where context anxiety is a problem (e.g., was essential for Claude Sonnet 4.5)

**HANDOFF.md template:**
```markdown
# Handoff Context

## What's been built
- [list of completed features]

## Current state
- App is running on port 3000
- Last commit: [hash] — [message]
- All passing tests: [count]

## Known bugs
- [specific bug at file:line]

## What to do next
- Next feature: [name from feature_list.json]
- Sprint contract is in SPRINT_CONTRACT.md

## Environment
- Run `./init.sh` to start dev server
- Run basic smoke test before touching new code
```

#### 3. Structured Note-Taking (Agentic Memory)
- Agent regularly writes notes to a persistent file outside context window
- Notes pulled back into context at later times
- Tracks progress, critical dependencies, strategic decisions
- Example: Claude playing Pokémon maintained step counts, maps, combat strategies across thousands of game steps
- Best for: iterative development with clear milestones
- Pattern: `NOTES.md`, `claude-progress.txt`, `todo.md`

#### 4. Sub-Agent Architecture
- Main agent coordinates with a high-level plan
- Subagents handle focused tasks with clean context windows
- Each subagent may use 10,000s of tokens internally but returns **condensed 1,000–2,000 token summary**
- Achieves clear separation of concerns
- Best for: complex research and analysis where parallel exploration pays dividends

**OpenClaw sub-agent example:**
```javascript
// Sub-agent does deep work, returns condensed summary
const result = await sessions_spawn({
  task: `Analyze the authentication module at apps/api/src/auth/.
  Test all endpoints for security vulnerabilities.
  Report findings as a concise 1000-word summary with specific file:line references.
  Do NOT return raw code or full file contents — summarize findings only.`,
  runtime: "subagent"
});
```

---

## Evaluation Criteria Templates

### For Full-Stack Applications (from Anthropic's GAN harness)
| Criterion | Threshold | What it measures |
|-----------|-----------|------------------|
| Product Depth | 7/10 | Are features substantive or surface-level? |
| Functionality | 7/10 | Does everything work end-to-end? |
| Visual Design | 6/10 | Coherent aesthetic? Not generic/AI slop? |
| Code Quality | 6/10 | Clean, tested, no stubs? |

### For Frontend Design (Anthropic's exact criteria — design quality weighted highest)
| Criterion | Weight | What it measures |
|-----------|--------|------------------|
| **Design Quality** | HIGH | Coherent whole? Colors, typography, layout, imagery create distinct mood and identity? |
| **Originality** | HIGH | Custom decisions or template defaults? Penalize: purple gradients, stock components, AI patterns |
| Craft | medium | Typography hierarchy, spacing consistency, color harmony, contrast ratios |
| Functionality | medium | Can users find actions and complete tasks without guessing? |

> Note: Anthropic found Claude already scores well on Craft and Functionality by default. Emphasize Design Quality and Originality to push toward aesthetic risk-taking.

**Evaluator uses Playwright MCP to navigate live page** — navigates, screenshots, studies before scoring. This is not code review; it's live testing.

**Calibration:** Use few-shot examples with detailed score breakdowns to align evaluator judgment with your preferences. This reduces score drift across iterations.

### For Security-Critical Applications (SOS-specific)
| Criterion | Threshold | What it measures |
|-----------|-----------|------------------|
| Auth Enforcement | 9/10 | Every endpoint authenticated? JWT validated? |
| Data Isolation | 9/10 | Multi-tenant? No cross-org leaks? |
| Input Validation | 8/10 | Parameterized queries? XSS prevention? |
| Audit Trail | 8/10 | All mutations logged with actor + timestamp? |

---

## Key Failure Modes and Solutions

| Failure Mode | Root Cause | Solution |
|--------------|------------|----------|
| Agent declares victory prematurely | Sees partial progress, assumes done | Initializer writes feature_list.json (all `passes:false`). Coding agent reads the list; can only mark `passes:true` after careful testing |
| Agent one-shots the whole app | No decomposition | Feature-by-feature approach; pick ONE feature from list each session |
| Half-implemented features left in codebase | Session ends mid-feature | Require git commit + progress update at end of every session. Smoke test at start of next session |
| Context anxiety — agent wraps up prematurely | Context window filling | Context resets (not just compaction) for models that exhibit this (was critical for Sonnet 4.5) |
| Evaluator approves bad work | LLMs are lenient toward LLM output | Tune evaluator with few-shot examples, read logs, find divergences, update prompt to be more skeptical |
| Generator self-evaluates positively | Same model can't objectively critique itself | Separate evaluator agent with independent, skeptical prompt |
| Tests pass but features broken | Claude tests with curl/unit tests but misses end-to-end | Require browser automation (Puppeteer/Playwright) — test as a human user would |
| Feature list gets modified | Agent edits instead of only marking passes | Use JSON (not Markdown). Add "strongly-worded instructions": "It is unacceptable to remove or edit tests as this could lead to missing or buggy functionality." |
| Lost context on session restart | No state persisted | `claude-progress.txt` + git log + `feature_list.json` as sources of truth |
| Agent can't figure out how to run the app | No init script | Initializer writes `init.sh` — run the dev server with one command |

---

## Anti-Patterns

| Don't Do This | Do This Instead |
|--------------|-----------------|
| Let generator evaluate its own work | Separate evaluator agent |
| Evaluate by reading code | Evaluate by using the live app via browser automation |
| Build all features at once | One feature → evaluate → next |
| Use compaction alone for models with context anxiety | Context reset with HANDOFF.md |
| Over-specify implementation details in spec | Specify deliverables, let generator choose path |
| Accept evaluator's first PASS without tuning | Calibrate evaluator with few-shot examples; read logs |
| Skip evaluation on "simple" changes | Evaluate everything against hard thresholds |
| Use Markdown for feature tracking | Use JSON (less likely to be edited by the model) |
| Load all data into context upfront | Just-in-time retrieval via file paths + grep/glob |
| Build complex harness from the start | Start minimal; add complexity only when needed |
| Keep old harness components when model improves | Re-examine every component when new model drops |
| Hardcode if-else logic in system prompts | Use right-altitude guidance with heuristics |
| Bloated tool sets with overlapping functionality | Minimal viable tools; each tool unambiguous |
| Staff exhaustive edge-case lists in prompts | Curate diverse, canonical few-shot examples |

---

## Running the Full Harness (OpenClaw)

### Step 1: Planner

```javascript
// In your main orchestration script
const plannerResult = await sessions_spawn({
  task: `Read USER_PROMPT.md. Expand into:
  1. SPEC.md — full product spec, design language, AI feature opportunities
  2. feature_list.json — all user-testable features, all passes:false
  3. init.sh — command to start dev server
  Be ambitious. Focus on WHAT, not HOW.`,
  runtime: "subagent",
  cwd: "/path/to/project"
});
```

### Step 2: Sprint Contract Negotiation

```javascript
// Generator proposes contract
await exec({ command: `droid exec --auto high -f .factory/propose-contract.md` });

// Evaluator reviews contract (sessions_spawn)
const contractReview = await sessions_spawn({
  task: `Read SPRINT_CONTRACT.md. Review the proposed sprint work.
  Does it match SPEC.md? Are the test criteria specific and testable?
  Approve or request changes. Write response to SPRINT_CONTRACT_REVIEW.md`,
  runtime: "subagent"
});
```

### Step 3: Generator Builds

```bash
cat > .factory/build.md << 'EOF'
1. Run ./init.sh
2. Run smoke test (browser: open app, perform core action, verify works)
3. Fix any pre-existing bugs
4. Read SPRINT_CONTRACT.md — implement agreed features
5. Self-evaluate: does the implementation match the contract?
6. git commit with descriptive message
7. Update claude-progress.txt
8. Write READY_FOR_QA.md with what was built
EOF

droid exec --auto high --model "custom:Claude-Opus-4.6-(OAuth)-0" -f .factory/build.md
```

### Step 4: Evaluator Tests

```javascript
const evalResult = await sessions_spawn({
  task: `You are a skeptical QA engineer. App is at http://localhost:3000.
  Read SPRINT_CONTRACT.md. Test every criterion using the browser tool.
  Navigate the app. Click buttons. Submit forms. Check data persists.
  Score 1-10 with evidence. FAIL if any criterion below threshold.
  Write EVAL_REPORT.md.`,
  runtime: "subagent"
});
```

### Step 5: Iterate or Advance

```javascript
const report = fs.readFileSync('EVAL_REPORT.md', 'utf8');
const passed = report.includes('OVERALL: PASS');

if (!passed) {
  // Feed feedback to generator for another round
  // If same approach failing after 2+ rounds, instruct generator to pivot
  iterationCount++;
  if (iterationCount > 5) {
    // Escalate to human — harness hit its limit
  }
} else {
  // Advance to next feature
  markFeaturePassing(currentFeature); // Update feature_list.json
}
```

---

## When to Simplify

> "Every component in a harness encodes an assumption about what the model can't do on its own, and those assumptions are worth stress testing."
> — Anthropic Engineering Blog

**When a new model is released:** Re-examine every harness component. Strip what's no longer load-bearing. Add new pieces to capture new capability headroom.

| If... | Then... |
|-------|---------|
| Task is within model's native capability | Skip the planner; go straight to generator |
| Simple bug fix with clear repro | Skip planner; skip sprint contract |
| Model reliably passes QA on these tasks | Reduce evaluator to single end-of-build pass |
| Model handles long sessions without anxiety | Drop context resets; use auto compaction |
| Model improved (new release) | Re-examine every component; strip dead weight |

**The evaluator is not a fixed yes-or-no.** It adds real value when tasks sit beyond what the current model handles reliably solo. As models improve, that boundary moves outward. Some tasks previously needing the evaluator will now be within native capability — and for those, the evaluator is unnecessary overhead.

**The Anthropic principle:** "Find the simplest solution possible, and only increase complexity when needed." (From [Building Effective Agents](https://www.anthropic.com/research/building-effective-agents))

---

## Cost Reference (from Anthropic's experiments)

| Configuration | Duration | Cost | Notes |
|---------------|----------|------|-------|
| Solo agent (Opus 4.5) | 20 min | $9 | Simple retro game maker — broken core feature |
| Full 3-agent harness (Opus 4.5) | 6 hours | $200 | Same prompt — working game with 16-feature spec |
| Updated harness (Opus 4.6, no sprint construct) | ~4 hours | $124 | DAW with working audio, mixer, agent integration |

The 20x cost difference produced clearly superior output. But model improvements (4.5 → 4.6) enabled removing the sprint construct entirely, reducing cost and complexity without degrading results.

---

## Appendix: Session Startup Transcript (from Anthropic research)

This is what a well-configured coding agent session startup looks like:

```
[Assistant] I'll start by getting my bearings and understanding the current state of the project.
[Tool Use] bash: pwd
[Tool Use] read: claude-progress.txt
[Tool Use] read: feature_list.json
[Assistant] Let me check the git log to see recent work.
[Tool Use] bash: git log --oneline -20
[Assistant] Now let me check the init.sh script to restart the servers.
[Starts the development server via init.sh]
[Assistant] Excellent! Now let me navigate to the application and verify that fundamental features are working.
[Tests basic functionality via browser automation]
[Assistant] Based on my verification testing, I can see that fundamental functionality is working well.
Now let me review feature_list.json comprehensively to understand what needs to be implemented next.
[Starts work on a new feature]
```
