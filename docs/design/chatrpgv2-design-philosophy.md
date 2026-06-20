# chatrpgv2 Design Philosophy

> Authoritative product-quality intent for the solo-GM TRPG experience. This document
> states **MUST** rules that the layered runtime (设计4补充.md 蓝图 §二 constitution) is
> required to honor at the *experience* layer, not only the *mechanics* layer. The owner
> review of the Triangle "The Vault" 战报 (2026-06-20) promoted §2b.1 and §2b.2 from
> implicit guidance to explicit MUSTs. These are blocking acceptance criteria for the
> Final Campaign Exam (FRAMEWORK §6).

---

## §1 What chatrpg is

chatrpg is a **solo tabletop-RPG referee**, not a choose-your-own-adventure menu engine and
not a co-author that writes the player's character for them. The player drives a single
protagonist through an authored module; the system plays every other role — the world, the
NPCs, the rules, and the narrator — through cooperating layers (Director / World / Rules /
Narrator / Policy / Kernel) rather than a single monolithic GM prompt.

The product is judged as a **game experience**: prose quality, mechanical honesty, NPC
consistency, story coherence, pacing, and — above all — **genuine player agency under a
fair, rules-first referee**. "It didn't crash" and "the tests pass" are necessary, not
sufficient.

---

## §2 Experience MUSTs

### §2a Language & rendering

- **§2a.1 Single language per session.** GM-facing output is rendered in **one** language,
  matching the campaign/player language. For the exam runs the language is **Chinese**, and
  EN/CN mixing within a session is a **blocking** defect. The player typing in Chinese must
  never receive English GM prose.

- **§2a.2 One unified roll-render convention.** A mechanical check is shown to the player in
  exactly **one** convention end-to-end. `[roll]…[/roll]` (or the project's single chosen
  form) is **reserved for a REAL mechanical check** — it MUST wrap an actual dice resolution
  with a shown outcome. Pure narration, flavor, or resource bookkeeping (e.g. "Chaos +1")
  MUST NOT be wrapped in a roll tag. An empty `[roll]` block (a roll tag containing no
  dice/check) is a **blocking** defect, and the runtime MUST guard against it.

### §2b GM craft (promoted to MUST, 2026-06-20)

#### §2b.1 Choices and clues via dramatic IMPLICATION — never menus or content dumps

The GM presents possibility and information **diegetically**, through the fiction. It is
**forbidden** to:

- **Offer explicit option menus.** e.g. "你现在可以立刻选择其一:A) … B) … C) …". The player
  decides what to do from the *situation*, not from a list the GM hands them.
- **Dump raw module content.** e.g. "你很快摸到几样关键东西:① … ② … ③ …", or reciting a
  clue's authored text as an inventory. Findings are not a bulleted manifest.

Instead the GM **MUST**:

- Convey choices as *affordances implied by the scene* — what is present, what is tense,
  what an NPC's body language invites or forbids.
- Surface clues woven into the fiction: an NPC's abnormal reaction, a strange expression, a
  meaningful glance, a detail that doesn't fit. **Show, don't tell.** A discovered clue
  becomes part of a described moment, not a labeled list item.
- Let the *player* draw the inference. The GM states what is perceivable; the player decides
  what it means and what to do.

The product-quality assessment MUST flag any explicit option menu or raw module-content dump
as a **blocking** defect.

#### §2b.2 The GM is a REFEREE, not a yes-man

This is the deepest rule and the most common failure mode (the cause of the shallow Triangle
27-turn "win", where the GM largely accepted the player's declarations and followed along
into a player monologue).

**The player declares INTENT, not OUTCOME.** "I shoot the guard" / "I persuade her" / "I pick
the lock" states what the character *attempts*. It does **not** state that it succeeds, nor
how the world responds.

For any action that is **uncertain, opposed, or risky**, the GM MUST act as a fair referee:

1. **The World resists.** The relevant NPC / faction / clock / environment reacts on its own
   terms (per the World layer and the module), independent of what the player wishes. The
   world does not bend to the declaration.
2. **The Rules adjudicate.** A real mechanical check is run against the character's **real
   parameters** (see §2c) and the real opposition/difficulty. The dice and the params decide.
3. **The outcome MAY NOT match the player's wish.** Failure, partial success, and
   costly/complicated success are all valid and should be **frequent**. A clean success is
   earned, not assumed.

The GM MUST NOT:

- Rubber-stamp a player-declared outcome ("you succeed" because the player said they do).
- Narrate an opposed/risky action resolving in the player's favor without world resistance
  and rules adjudication.
- Let trivial, unopposed actions consume a check — *only* uncertain/opposed/risky actions are
  adjudicated; the rest just happen (no empty `[roll]`, per §2a.2).

The product-quality assessment MUST flag, as **blocking** defects: "GM rubber-stamps
player-declared outcomes", "no world resistance on an opposed/risky action", and "no
adjudication on an uncertain action".

#### §2b.3 Visibility markup — the GM may emit `[system]`/`[hide]`; presentation strips them

GM output that must **not** reach the player but **does** affect continuity is tagged, and the
presentation layer strips the tag from the player's view while keeping the content where it
belongs:

- **`[system]…[/system]` = out-of-game meta** — mechanical-resolution exposition, dice math,
  GM reasoning. The player sees only the fiction outcome, never the meta. This is the clean
  route for "数值65 掷63 通常成功"-style exposition that must not leak into prose. Routed to
  the engine/audit, stripped from player display.
- **`[hide]…[/hide]` = in-fiction events that happened but THIS player did not perceive** —
  off-screen NPC actions, secret-roll (暗骰) triggered events. They go to campaign-canon /
  world-state (stay consistent, future-referenceable) but **not** to player-knowledge, and are
  not rendered to the player this turn.
- Presentation **strips both** from the player display; `[roll]…[/roll]` is reserved for a real
  mechanical check (per §2a.2) and an empty `[roll]` wrapper is unwrapped (its inner prose kept,
  the tag removed). This reuses the existing visibility filter / gm_only / known-to-player gate
  — a GM-output markup face, not new infra.

Validate: the player transcript shows **zero** `[system]`/`[hide]` content and zero raw
mechanical exposition; the context/DB retains it (continuity preserved).

### §2c Character parameters are real and they drive the dice

A character is defined by **mechanical parameters** compiled from the ruleset + the authored
sheet — not by a bare label. Chargen MUST compile the ruleset's competency model (ratings,
attributes, skills, and any pool-building rule) into mechanical params via the formula/chargen
compiler, with **no hardcoding** and no ruleset_id/module_id name-branching.

Those params MUST **drive resolution**: the dice pool / target number / modifier a check uses
is **derived from the character's params**, not flat. A competent character and an incompetent
character attempting the same thing MUST roll differently. A "flat 6d4 regardless of who you
are" is a **blocking** defect. This is the mechanical foundation that makes §2b.2 adjudication
meaningful — the referee adjudicates against *real* competence.

### §2d Every adjudicated turn narrates

When a check runs, the turn MUST produce real narrative prose conveying the fiction of the
result — never a bare mechanical line, raw JSON echo, or empty/near-empty narration. A
check-ran-but-empty-narration turn is a **blocking** defect; the verify/repair pipeline MUST
catch it and force re-narration.

---

## §3 Relationship to the constitution

These experience MUSTs sit **on top of** 设计4补充.md §二 (12 architecture rules) and do not
relax any of them. In particular: §2b/§2c are realized through the existing layers (World
produces reactions; Rules adjudicates; Narrator expresses; Director paces; Kernel commits) —
never by inflating a single GM prompt or by a layer overstepping. All additions are additive,
flag-gated, OFF==baseline byte-equal, source-backed, data-driven, and fail-closed, consistent
with the 叠加操作原则.
