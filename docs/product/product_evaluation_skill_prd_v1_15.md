# Product Design: v1.15 Product Evaluation Skill

## What it is

A Claude Code / subagent skill that evaluates chatrpg as a complete solo LLM-GM TRPG product. It tells an evaluator agent how to test the product journey, how to score GM quality, how to run black-box playtests, and how to write an evidence-based evaluation report.

## Why it exists

Claude Code naturally tends to test a repository like software: build, run unit tests, inspect functions, and check logs. chatrpg needs a different kind of test: it must prove that a normal player can upload a rulebook and module, create a character, and enjoy a session where the LLM GM behaves as a referee and scenario runner.

## User story

As the project owner, I can ask Claude Code:

> Use the chatrpg-product-evaluator skill and evaluate this build.

The agent will then run build/harness checks, conduct or inspect a product playtest, score the system using a product rubric, and produce a report with evidence and next steps.

## Success criteria

- Evaluator does not stop after `cargo check` or harness phase PASS.
- Evaluator tests a playable session slice.
- Evaluator verifies DB/event state when mechanics are claimed.
- Evaluator scores UX/fun/NPC quality, not only functional correctness.
- Evaluator tests weird, multilingual, and creative player actions.
- Report contains severity-ranked findings with reproductions and evidence.

## Non-goals

- This does not make Claude Code the production GM.
- This does not replace OpenAI/OpenAI-compatible API runtime backends.
- This does not grant evaluator agents permission to alter production state during a live game.
