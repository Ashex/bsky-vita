# Implementation plans

## Execution order and status

| Plan | Goal | Priority | Depends on | Status |
|---|---|---|---|---|
| [001](001-pocketjs-live-feed-poc.md) | Live PocketJS timeline, validate migration gaps | P1 | None | Architecture recovery in progress; original live-feed work not accepted |

Plan 001 retains its original staged proposal, but its superseding recovery amendment now governs architecture, scope, sequencing, and acceptance for recovery. Architecture work is in progress; the original live-feed work is not accepted. Hardware validation remains separate and deferred.

## Model assignments

- Implementer: OpenAI `openai/gpt-6-luna`.
- Reviewer: OpenAI `openai/gpt-6.1-sol`.
- Supervisor: OpenAI `openai/gpt-6-astra` (user-confirmed).

Use exact models, no silent substitutions. Astra owns scope, admission and evidence; GPT-6 Luna is the sole source writer; Sol independently reviews actual diffs. Implementation requires a subsequent execution directive. No commits, pushes, merges or device deployment are implied.

## Workspace

Use `/home/evelyn/git/atprotocol/bsky-vita` on implementation branch `feat/pocketjs-live-feed-poc`, created before source edits. Preserve the original dirty baseline recorded in plan 001. No routine worktree is needed.

## Dependencies and rejected shortcuts

Establish Vita compile/link and one renderer owner before bridge/UI work. Connect the live worker before claiming a usable prototype. Fixtures supplement live data. Uniform-height lists, baked Latin-only text, a hidden native timeline rendered as one image, and CPU goldens offered as GPU evidence are not acceptable substitutes.

## Plan-review status

Sol (`openai/gpt-6.1-sol`) initially requested changes for containment, auth generations, preallocation bounds, moderation, lifecycle and hardware admission. Astra (`openai/gpt-6-astra`) assessed CONDITIONAL GO for milestone 0 after amendments, adding scope, whole-process budgets, bridge validation and fallback requirements. These are incorporated in plan 001's mandatory review amendments. Sol re-reviewed the amended plan and returned PASS.

Astra's assessment read plan 001 and the repository root README, not this index. The review establishes plan readiness only. Runtime implementation, build results and hardware acceptance remain TODO. No GPT-6 Luna implementation has been dispatched.
