<!-- yah:begin (managed by yah, do not edit between markers) -->
## KG slice

### `<work-item>`
- ticket `ticket:R017-F1` — line 0
- ticket `ticket:R016-F1` — line 0
- relay `relay:R017` — line 0
- relay `relay:R016` — line 0

### `packages/mesofact-build/src/index.ts`
- file `packages/mesofact-build/src/index.ts` — line 1
- fn `packages/mesofact-build/src/index.ts::defaultBuildId` — line 304
- fn `packages/mesofact-build/src/index.ts::parseListQuery` — line 294
- fn `packages/mesofact-build/src/index.ts::expandPrerenderParams` — line 272
- fn `packages/mesofact-build/src/index.ts::build` — line 105
- ts `packages/mesofact-build/src/index.ts::BuildResult` — line 97
- ts `packages/mesofact-build/src/index.ts::BuildOptions` — line 86

### Cross-references

- anchors: 4
- calls: 3
- contains: 6
- parent_item: 2

## Arch doc: .yah/docs/working/W173-mesofact-render-cube.md

_Reference: `.yah/docs/working/W173-mesofact-render-cube.md` (not pre-loaded)._

## yah SDLC — source-embedded tickets

Work items live as `@yah:` doc-comment annotations in source. There is **no
separate issue tracker**. Launch the kanban UI with `yah board serve` (it
auto-picks a port from the workspace path).

### Lifecycle

| Column | `@yah:status(...)` | Meaning |
|---|---|---|
| **Quests** | (derived) | Coordination relays that own child relays — see below |
| **Open** | `open` | Unclaimed — also holds `.yah/todo.md` entries (pre-ticket inbox) |
| **Active** | `claimed` or `in-progress` | Someone's working on it |
| **Handoff** | `handoff` | Ready for next agent — use `/handoff` |
| **Review** | `review` or `done` | Awaiting sign-off |

Tickets move between columns by editing their `@yah:status(...)` line in
source *or* by drag-and-drop on the UI (the server rewrites the status
line for you under the same transition matrix). Allowed transitions:

- `open → active`
- `active → open | handoff | review`   (`active → open` is the admin undo)
- `handoff → active | review`
- `review → handoff`

Anything else is refused (UI dims the target column; server returns 409).

### SDLC rules

Run `yah board rules` (or the `board.rules` MCP tool) for the canonical
ruleset (Rule01–Rule12 + Col01). Narrow to a situation with
`--context pickup | finishing | new-work | archive | refactor` — or use
`--format terse` for one-line rules. For a planning-agent snapshot
(counts, active owners, handoff queue, smell), run `yah board status`.

High-leverage rules to remember without looking:

- **Rule01** — first edit on pickup is `@yah:status(in-progress)` on the ticket
- **Rule03** — finishing a phase updates the *existing* relay in place (same R-number);
  new R-numbers only for parallel/independent tracks
- **Col01** — three end-states: more work → **Handoff** (same relay, Rule03);
  tasks met but unverified → **Review** + ping user; human signed off →
  archive. Never self-archive on the same turn you set review, and never
  drop finished work back into Active.
- **Rule04** — `status(done)` is staging; archive is the terminal action

### Quests

A quest is a coordination point, not a unit of work. Declare one with
`@yah:kind(quest)` on a relay (`yah board open --kind quest …` allocates a
`Q<n>` ID, e.g. `Q005`); the board also *infers* quest-ness from any relay
that has **bare-R or bare-Q child relays** pointing at it via
`@yah:parent(...)`. Compound sub-tickets (`R007-T1`) never promote their
parent to a quest. The legacy `@yah:kind(epic)` is accepted as an alias.

Tasks/features/bugs **cannot be parented directly to a Q-id quest** —
quests own relays, not leaf tickets. Open a child relay under the quest
first, then attach sub-tickets to that relay.

R-relays and Q-quests share one counter, so a quest can't reuse a number
already taken by a relay (or vice versa) — the prefix shows intent
(work vs. coordination), not a parallel ID space.

Quests get a computed status:

- **active** — at least one child relay is still live (not in `review`)
- **closed** — all children reached `review`/`done` (or have been archived)

Quests live in their own leftmost column. Their own `@yah:status(...)` is
ignored once they qualify as a quest. Archiving a quest while it still has
live children returns a 409 — archive the children first.

### First action on pickup

When an agent claims a ticket, the **first edit** is setting
`@yah:status(in-progress)` on that ticket and saving. That is the claim
signal. Don't start modifying other code until the status line is updated.

### Archiving (not "done")

Tickets don't stay on the board after they ship. Click the `archive` button
on the ticket card — that strips the `@yah:…` annotation lines from source
and appends an audit record to `.yah/events.jsonl`. Treat `status(done)` as
a short-lived staging state, not a resting place.

### The event log

`.yah/events.jsonl` is a derivative audit log (not the source of truth):
`created`, `modified`, `archived`, `disappeared`. The server replays it on
startup and diffs against current source, so tickets that get accidentally
deleted ("clobbered") surface as `disappeared` events and can be restored
from the last-known snapshot.

### Slash commands

- `/comment` — log a progress summary to `.yah/summaries/`
- `/handoff` — write a structured relay for the next agent (`@yah:relay(...)`)
- `/refine` — turn a multi-phase plan into a relay + tickets

If the slash commands aren't available in your harness, each prompt is
also reachable as `yah board prompt <name>` — same content, no install
required.

### Never pick IDs yourself

Two agents running in parallel will race and both pick the same R-number.
Use `yah board open` (file for later) or `yah board claim` (start now) —
both take a file lock, scan source for the next unused ID, and write the
annotation atomically:

```bash
# File for later (Open column, no assignee):
yah board open --kind bug --parent R065 \
  --file packages/yah/ui/src/foo.tsx --title "Short title" \
  --next "First concrete step"

# Start now (Active column, assigned to claude):
yah board claim --kind relay \
  --file src/module.rs --title "Short title" \
  --assignee agent:claude \
  --next "First concrete step"
```

Stdout is the new ID. Two shapes:

- **Bare relay** (`--kind relay` without `--parent`) → `R008`
- **Compound sub-ticket** (`--kind task|feature|bug|spike` with `--parent R007`) →
  suffix mirrors kind: `-T` task, `-F` feature, `-B` bug, `-S` spike. All four
  share one per-relay counter — numbers stay monotonic across kinds.

`--parent` is required for `--kind task|feature|bug|spike` *(when used as
sub-ticket)*. `--kind spike` *without* `--parent` is the exception — it opens
a top-level R-prefixed relay flagged as exploratory. Orphan bare IDs
(`T01`, `F01`, `B01`, `S01`) are rejected: they collide with compound
sub-ticket numbering. For one-off work, `board open --kind relay …` first
and attach the task under it.

**Open or claim a sub-ticket inside the current relay**, don't spin up a
new relay for every chunk. The relay is the baton; sub-tickets are the
incremental checkpoints.

### Card actions

Each ticket card has two small buttons in the top-right:

- **prompt** (or **review** when the card is in the Review column) — copies
  a continuation prompt to the clipboard. For review-column cards the
  prompt is review-mode (verify + approve-or-send-back); for open/handoff
  it's a pickup prompt (`board tickets --prompt <ID>` output).
- **archive** — click once to arm (surfaces `@yah:verify(...)` commands if
  any), click again to commit.

### Where annotations go

The scanner parses Rust with `syn` and only reads doc comments attached to:

- **Module-level** (`//!` at file top, or inside `mod foo { //! … }`)
- **Top-level items** via `///` — `struct`, `enum`, `fn`, `impl` blocks, `mod`

It does **not** read `///` on enum variants, struct fields, methods inside
`impl` blocks, consts, statics, type aliases, or trait items. An annotation
placed there is invisible to the board.

**Default to `//!` at the top of the file.** Use item-level `///` only when
the ticket genuinely tracks one specific top-level item.

### Key annotations

- `@yah:ticket(ID, "title")` / `@yah:relay(ID, "title")` — define the item
- `@yah:kind(feature|bug|task|spike|quest|epic)` — override kind
- `@yah:status(open|claimed|in-progress|handoff|review|done)` — column
- `@yah:assignee(agent:name)` — who's working on it
- `@yah:phase(P1)` / `@yah:parent(R001)` — ordering / hierarchy
- `@yah:handoff("…")` — message for the next agent
- `@yah:next("…")` — concrete next step (repeatable)
- `@yah:verify("…")` — how to confirm done (repeatable; rendered as fenced bash + `&&` smoke test)
- `@yah:gotcha("…")` — pre-existing breakage / traps for the next agent (repeatable)
- `@yah:assumes("…")` — unverified claim baked into the handoff (repeatable)
- `@yah:cleanup("…")` — deferred tech debt (repeatable)
- `@yah:depends_on(ID)` — declare a dependency (cycle detection surfaces as a smell)
- `@arch:see(path/to/doc.md)` — link to architecture docs

## Output conventions

When you reference a file, function, or symbol the user might want to jump to, prefer markdown links with the `yah://` scheme over bare paths:

- `[path/to/file.rs:42](yah://file/path/to/file.rs#L42)` — opens the file in the Architecture tab rooted at that line.
- `[Foo](yah://arch/symbol/Foo)` — re-roots the arch graph on the named symbol.

The renderer turns these into clickable affordances; bare backticked `path:line` chips also work but yah:// links are preferred for prose.

## Board tools

Board MCP tools are namespaced `board.*` (dots, not underscores) — call them directly when present in your tool list; fall back to `yah board …` via Bash otherwise. The tool schemas describe their own arguments — trust those over any table.

Two semantic rules the schemas can't tell you:

- **Move into `handoff`:** update `@yah:handoff(...)` and `@yah:next(...)` annotations in source *first*, then call `board.move {"id": "<ID>", "to_bucket": "handoff"}`. The baton moves with the source, not the card.
- **Read tools** (`board.show`, `board.list_tickets`, `board.list_relays`, `board.ticket_prompt`, `board.validate`, `board.status`, `board.rules`, `board.summary`) auto-pass the approval gate. **Write tools** (`board.claim`, `board.open`, `board.move`, `board.archive`, `board.update`, `board.promote_next`, `board.promote`, `board.comment`) route through it.

## Environment quirks

- **`mcp__yah__ask_user`** is the canonical user-choice affordance: use it for structured multiple-choice prompts (multi-option, multi-select, or multi-question forms). Do NOT use it for single free-form questions — just print those into chat. `AskUserQuestion` is not wired up in this host.
- **Tool-use approvals** (Bash, Write, etc.) route through the AnswerQueue UI via `--permission-prompt-tool mcp__yah__approve_tool`; a Continue/Revise modal will appear in the desktop panel. To minimize Revise round-trips: name the target in the call's `description` ("Read app/yah/cli/src/main.rs" beats "Read file" — the user pattern-matches on description before clicking Continue); scope paths narrowly (`rg "foo" crates/yah/board/` is approvable, unbounded `rg "foo"` is a Revise); don't pre-stage destructive shapes (`rm -rf`, `git reset --hard`, `find … -delete`, `--no-verify`) unless the user has authorized that exact operation — they escalate to a hard review even when the target is harmless.
- **Grep `type: "tsx"` returns zero results silently.** claude-cli's Grep wraps ripgrep, which only knows `ts` (covers `.ts` and `.tsx`). Use `type: "ts"` or `glob: "**/*.tsx"`. If a Grep you expect to match returns nothing, recheck the type field before concluding the pattern is absent.

## Subagent delegation

To delegate exploration, search, or analysis to a cheaper subagent, call `mcp__yah__subagent_spawn`. Three dispatch tiers — pick the narrowest that fits:

1. **`character`** — specific party member the user named (e.g. `@quill`). Use only when the user asked for someone by name.
2. **`subclass`** — capability tag (e.g. `"explorer"`, `"searcher"`, `"analyst"`). Use when delegating by capability.
   - `subclass: "explorer"` — Read/Grep/Glob; cheap; file discovery.
   - `subclass: "searcher"` — Grep/Glob only; cheapest; single-pass search.
   - `subclass: "analyst"` — Read/Grep/Glob; mid tier; synthesis across many files.
3. **Neither** — auto-dispatch: yah picks the best subclass for the `job`. Add `hints` to narrow the candidate set:
   - `hints: ["cheap"]` — prefer lowest-cost subclass (soft rank).
   - `hints: ["reasoning"]` — require extended thinking capability (hard filter).
   - `hints: ["large_context"]` — require ≥32K context window (hard filter).

**Never pass a sigil** (e.g. `yah-quill-0`) — yah allocates slots; agents must not name them. The spawn receipt returns `{ session_id, slot_slug }` — use `session_id` for all follow-up calls. Call `mcp__yah__subagent_roster {}` to see bookable characters and subclasses.

The subagent runs as a separate yah session — the user sees it in their party and can follow along.

After the subagent finishes, read its structured findings with `mcp__yah__subagent_corpus_shape { session_id }` (substrate inventory) and `mcp__yah__subagent_query { session_id, substrate: "markdown", query: "<pattern>" }` (substring search across notes).

**If you are a subagent** (launched to serve another agent): write your findings as `.md` files to `.yah/subagents/<session_id>/notes/` using `write_arch_doc` or `edit_file` before your final turn. Your session id is in the `YAH_SESSION_ID` environment variable.

# Ticket: R017 — Parametric prerender enumeration from local data_inputs

- **id**: `R017`
- **status**: open
- **source**: `packages/mesofact-build/src/index.ts:1`
- **slot**: slot:bundle-anthropic-ashguard:2

## Gotchas (read first)

- Don't confuse with the existing `from`/query/param shape — that one walks a registered source adapter (R2 BlobSource) via async load. from_data is intentionally synchronous + local-file-only, riding the data_inputs read that already happens at prerender.ts:114-118.

## Next steps

- Parametric routes today enumerate IDs either via literal `prerender:{params:[...]}` or via a registered R2-shaped source adapter (BlobSource). There's no path to enumerate from a local-JSON data_inputs file at build time. This relay adds a third shape.
- Single feature unit — see child F-ticket. Independent from the Cell 2 relay; either can ship first.
- Real consumer: yah-camp R443-F2's /issues/:id wants one static HTML per issue, enumerated from src/data/issues.json (the same file feeding data_inputs).

## Assumptions (challenge if wrong)

- Naming the new field `from_data` (rather than reusing `from`) is the right disambiguation — `from` already means 'registered async source adapter', from_data means 'local JSON file already declared in data_inputs'. Verify with one consumer before locking the name.
<!-- yah:end -->
