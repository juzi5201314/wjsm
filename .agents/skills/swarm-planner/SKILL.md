---
name: swarm-planner
description: "Auto-generate and run swarm pipeline YAML from a high-level goal. Use when the user asks to 'implement X', 'build X', 'do X with swarm', or gives any complex multi-step task that benefits from parallel or staged agent orchestration. Also use when the user says 'swarm', 'pipeline', 'multi-agent', or describes work that naturally decomposes into independent specialist agents (research, coding, testing, review, data processing, content creation). If the task involves 3+ distinct phases or could benefit from parallel specialists, this skill applies."
---

# Execute

→ User describes a complex goal? → **Decompose into agents, generate swarm YAML, run it.**
  1. Analyze goal → identify independent sub-tasks and dependencies
  2. Choose execution mode (sequential / parallel / pipeline)
  3. Generate swarm YAML to `swarms/<name>.yaml`
  4. Create workspace directory
  5. Run `/swarm run swarms/<name>.yaml`

# Swarm Planner

Generate swarm pipeline YAML from a high-level goal and execute it. You are the planner — the user provides intent, you produce the orchestration.

## When to Use

Use when a task naturally decomposes into multiple specialist agents working in coordination. Typical triggers:

- "Implement a user auth system" (DB schema → API → frontend → tests → integration)
- "Research and write a blog post about X" (research → write → edit → review)
- "Audit this codebase" (security + performance + architecture → lead summary)
- "Build a data pipeline" (scrape → transform → validate → load)
- "Run this task 20 times" (iterative collection/processing with pipeline mode)

**Don't use when:**
- The task is a single-file edit or simple question (just do it directly)
- The task has fewer than 2 distinct phases
- The user explicitly wants to write the YAML themselves

## The Process

### 1. Analyze the Goal

Before generating anything, decompose the user's goal into:

- **What are the distinct phases?** (e.g., research, implement, test, review)
- **What can run in parallel?** (independent specialists with no data dependency)
- **What must be sequential?** (later phases that read earlier phases' output)
- **Is iteration needed?** (batch collection, repeated processing → pipeline mode)

Present the decomposition to the user briefly:

```
Decomposition:
  Wave 1 (parallel): researcher + data-collector
  Wave 2 (sequential): writer
  Wave 3 (sequential): reviewer
  Mode: sequential | 1 iteration
```

Ask for approval if the decomposition is non-obvious. For straightforward cases, proceed directly.

### 2. Choose the Mode

| Mode | When to use |
|------|-------------|
| `sequential` | Linear pipeline: A → B → C. Default for most implementation tasks. |
| `parallel` | Independent specialists, one synthesis step at the end. Good for audits, multi-perspective analysis. |
| `pipeline` | Repeat the whole graph N times. Good for batch data collection, iterative refinement. |

### 3. Generate the YAML

Write the YAML to `swarms/<name>.yaml` (create the `swarms/` directory if it doesn't exist). The name should be a short kebab-case identifier derived from the goal.

**YAML must conform exactly to this schema:**

```yaml
swarm:
  name: <kebab-case-id>           # [a-zA-Z0-9._-] only
  workspace: ./workspace          # relative to YAML file location
  mode: sequential | parallel | pipeline
  target_count: 1                 # only >1 in pipeline mode
  model: claude-opus-4-6          # optional, omit to use session default

  agents:
    <agent_name>:                 # snake_case identifier
      role: <short-role>          # becomes "You are a <role>."
      task: |                     # full instructions (user prompt)
        ...
      extra_context: |            # optional, appended to system prompt
        ...
      model: claude-sonnet-4-5    # optional per-agent override
      reports_to:                 # optional: downstream agents
        - <agent_name>
      waits_for:                  # optional: upstream agents
        - <agent_name>
```

**Validation rules** (the parser enforces these — your YAML must pass):
- `name`: only `[a-zA-Z0-9._-]`
- `workspace`: required string
- `mode`: one of `pipeline`, `parallel`, `sequential`
- `target_count`: ≥ 1, only meaningful in `pipeline` mode
- Every agent needs `role` (string) and `task` (string)
- `reports_to` and `waits_for` must reference existing agent names
- No self-references in dependencies
- No cycles in the dependency graph

### 4. Write Agent Tasks

Each agent's `task` is the most important field. Write it as if you're giving instructions to a colleague who knows nothing about the project. Be explicit:

**Input paths:** Tell the agent exactly where to read input from.
```yaml
task: |
  Read research/findings.md for the raw data.
```

**Output paths:** Tell the agent exactly where to write output.
```yaml
task: |
  Write the final report to output/report.md
```

**Format requirements:** If downstream agents parse the output, specify the format.
```yaml
task: |
  Write results as JSON to results/data.json with this schema:
  { "items": [{ "name": "...", "score": 0-100 }] }
```

**Failure handling:** Tell agents what to do when things go wrong.
```yaml
task: |
  If the source has no useful data, write SKIP to signals/out.txt
  and explain why in skipped/<agent>.md
```

**For pipeline mode (iterative):** Agents start fresh each iteration — they don't remember previous runs. Tell them to check tracking files:
```yaml
task: |
  Read processed.txt to see what's already done.
  Pick the next unprocessed item.
  Append it to processed.txt when done.
```

**Scope tightly:** One clear objective per agent. An agent that tries to do five things does zero well.

### 5. Design Inter-Agent Communication

Agents communicate through files in the shared workspace. Design the protocol:

**Signal files** (lightweight status flags):
```
signals/finder_out.txt    → "FOUND:https://example.com"
signals/writer_out.txt    → "DONE:drafts/post.md"
```

**Structured output** (detailed results):
```
research/findings.md     → Full research document
output/report.json       → Machine-readable data
```

**Tracking files** (prevent duplicate work in pipeline mode):
```
processed.txt            → Items already handled
tracking/count.txt       → Current iteration counter
```

### 6. Create Workspace and Run

```bash
# Create the workspace directory (relative to YAML location)
mkdir -p swarms/workspace

# Run via the TUI command
/swarm run swarms/<name>.yaml
```

For long-running pipelines, you can also run standalone:
```bash
nohup omp-swarm swarms/<name>.yaml > pipeline.log 2>&1 & disown
```

### 7. Monitor

After launching, check progress:
```
/swarm status <name>
```

Or read the state directly:
```bash
cat swarms/workspace/.swarm_<name>/state/pipeline.json | python -m json.tool
tail -f swarms/workspace/.swarm_<name>/logs/orchestrator.log
```

---

## Pattern Library

### Sequential Implementation

Linear build: each phase produces artifacts the next phase consumes.

```yaml
swarm:
  name: auth-system
  workspace: ./workspace
  mode: sequential

  agents:
    schema_designer:
      role: database-architect
      task: |
        Read spec.md for the authentication requirements.
        Design the database schema (users, sessions, tokens).
        Write SQL migration to migrations/001_auth.sql.
        Write schema documentation to docs/schema.md.

    api_developer:
      role: backend-developer
      task: |
        Read migrations/001_auth.sql and docs/schema.md.
        Implement REST API endpoints:
          POST /auth/register
          POST /auth/login
          POST /auth/logout
          GET  /auth/me
        Write to src/api/auth/.
        Write API docs to docs/api-auth.md.

    frontend_developer:
      role: frontend-developer
      task: |
        Read docs/api-auth.md for endpoint contracts.
        Implement auth UI components:
          Login form, Register form, Protected route wrapper.
        Write to src/components/auth/.

    test_engineer:
      role: test-engineer
      task: |
        Read docs/api-auth.md and src/api/auth/ and src/components/auth/.
        Write integration tests covering:
          Registration flow, Login/logout, Token refresh, Error cases.
        Write to tests/auth/.
        Run tests and fix failures.
```

### Parallel Audit

Independent specialists + one synthesizer. Use `reports_to` for fan-in.

```yaml
swarm:
  name: codebase-audit
  workspace: ./workspace

  agents:
    security:
      role: security-auditor
      task: |
        Audit all code in src/ for security vulnerabilities.
        Rate each finding: CRITICAL / HIGH / MEDIUM / LOW.
        Write to reports/security.md.
      reports_to: [synthesizer]

    performance:
      role: performance-analyst
      task: |
        Analyze src/ for performance bottlenecks.
        Identify: N+1 queries, unnecessary allocations, missing indexes.
        Write to reports/performance.md.
      reports_to: [synthesizer]

    architecture:
      role: architecture-reviewer
      task: |
        Review src/ for architectural issues: coupling, god files, unclear boundaries.
        Write to reports/architecture.md.
      reports_to: [synthesizer]

    synthesizer:
      role: engineering-lead
      task: |
        Read all files in reports/.
        Create a prioritized action plan in output/action_plan.md.
        Group by: Quick wins (< 1 day), Medium effort, Large refactors.
        Include estimated impact for each item.
      waits_for: [security, performance, architecture]
```

### Iterative Pipeline

Repeat the agent graph N times. Each iteration builds on the previous one's output.

```yaml
swarm:
  name: research-collector
  workspace: ./workspace
  mode: pipeline
  target_count: 20

  agents:
    finder:
      role: researcher
      task: |
        Read tracking/processed.txt to see topics already covered.
        Use web_search to find ONE new high-quality source on the topic.
        Append the URL and title to tracking/processed.txt.
        Write the URL to signals/url.txt (just the URL, one line).

    analyzer:
      role: analyst
      task: |
        Read signals/url.txt for the URL to analyze.
        Fetch the page content.
        Read tracking/count.txt, increment it, write back.
        Write analysis to analyzed/item_<N>.md (use the count).
        Write the item number to signals/item_num.txt.

    compiler:
      role: technical-writer
      task: |
        Read signals/item_num.txt for the latest item number.
        Read analyzed/item_<N>.md.
        Append a summary section to output/report.md.
```

---

## Decision Guide: Choosing Agent Granularity

**Too few agents** (1-2): You're not getting parallelism benefits. Just do it sequentially.

**Too many agents** (8+): Coordination overhead and communication complexity explode. Merge agents with tight coupling.

**Sweet spot** (3-6): Enough parallelism for speed, few enough for clean communication.

**Rule of thumb:** If two tasks share input files or must agree on an interface, they're one agent. If they can work completely independently and only meet at the output, they're two agents.

## Common Mistakes

**Forgetting to specify paths:** Agents don't know where to look. Always say "Read X, write to Y."

**Overlapping output:** Two agents write to the same file → corruption. Each agent writes to its own file; a synthesizer merges.

**Vague tasks:** "Do the thing" → agent hallucinates scope. Be specific about what "done" looks like.

**Missing error handling:** Agent hits an edge case, does nothing silently. Always tell agents what to do on failure.

**Pipeline mode without tracking:** Agent repeats work from previous iterations. Always include "Read processed.txt first."

**Circular dependencies:** A waits_for B, B waits_for A. The parser will reject this, but design your DAG to avoid it from the start.
