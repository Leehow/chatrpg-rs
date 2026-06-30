# CoC Complete Playtest Candidates

Source PDF:
`/Users/haoli/Documents/TRPG/coc英文/Call Of Cthulhu Keeper Rulebook 40th Anniversary (Sandy Petersen).pdf`

Metadata checked locally: 465 PDF pages, unencrypted. Title hits were verified
with `pdftotext` and spot-checked with the bundled `pypdf` runtime.

## Recommendation

Use **The Haunting** as the first complete CoC run under the J1-J4/Q4/P1/M0
evaluation scheme.

- PDF pages: 447-463.
- Book pages: 435-451.
- Shape: bounded investigative one-shot with a fixed haunted-house objective,
  several research locations, handouts, and a final house confrontation.
- Stated audience: new Keepers and players, with Keeper notes and dice guidance
  embedded in the scenario text.
- Expected test size: about 35-70 player turns for a cautious investigator,
  comfortably under the 100-turn target if the player simulator keeps goals
  focused.

Why it fits this evaluator:

- M0 can show character creation first; the scenario itself asks for newly
  created investigators.
- J1 continuity is easy to audit because the location graph is small:
  introduction, newspaper, library, records, police/courts, sanitarium, house.
- J2/J4 get real pressure from social access rolls, Library Use/Law research,
  sanity threats, and combat in the house.
- P1 can use the cautious-investigator persona without becoming a script loop:
  research first, check handouts, then inspect the house.
- Q4 has a clean product bar: the GM should present clues and affordances, not
  menu the research locations or house-room actions.

## Other Candidates

### Amidst the Ancient Trees

- PDF pages: 358-375.
- Book pages: 346-363.
- Shape: action-forward forest search/rescue scenario with tracking, stealth,
  firearms/fighting, a timeline, captives, and Mythos servants.
- Fit: good second complete CoC test once the evaluator needs wilderness
  pursuit, combat, and timeline pressure.
- Risk: more moving parties and wilderness branching make it easier to exceed
  the 100-turn budget if the player simulator is cautious.

### Crimson Letters

- PDF pages: 376-390+.
- Book pages: 364-382.
- Shape: open-ended Arkham murder mystery built around NPC motives, a Keeper-
  chosen culprit, missing papers, optional deadline pressure, and many social
  branches.
- Fit: strong later T3/T4 quality test for nonlinear investigation and NPC
  reasoning.
- Risk: it explicitly says it can reward multiple sessions and requires Keeper
  preparation decisions; not ideal as the first bounded 100-turn full run.

## Next Implementation Step

Do not hardcode the scenario into the engine. Onboard **The Haunting** as a
source-backed module slice from the local PDF, preserving page references and
player/keeper visibility. Then run:

1. `create-character --auto` and write `character_sheet.md`.
2. `opening` for the scenario.
3. Player simulator turns with full `PLAYER_DECISION` rows.
4. `coverage`, `redboard`, and `battle_report_with_character.md`.

First-pass completion criterion:

- M0/J1/J2/J3/J4/P1 are GREEN.
- Q4 has zero explicit option-menu hits.
- The report includes character creation, player-visible GM outputs, player
  inputs, checks/rolls, and final outcome.
