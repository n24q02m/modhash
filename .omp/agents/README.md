# `.omp/agents/` - the worker fleet

Project-scoped subagent definitions for this repository. OMP discovers them
from `.omp/agents/*.md` (see `omp://task-agent-discovery.md`), and rediscovers
them on every task dispatch, so adding or editing a file takes effect on the
next dispatch without a restart.

## Why definitions instead of configuration

A subagent's model is a property of the **agent**, not of the individual call.
The `task` tool has no per-call model parameter, so the only way to run four
models in parallel is to have four agents. Model precedence, in order:

1. `task.agentModelOverrides[agentName]`
2. this file's `model:` frontmatter
3. the parent session's active model

This directory implements route 2 deliberately: it is a file in the repo, so
it is reviewable, diffable and travels with the project, rather than living in
a machine-global config.

## The four lanes

| agent | model | owns | why this model |
|---|---|---|---|
| `modhash-bulk` | `openrouter/stealth/space-bunny-alpha` | docs, specs, fixtures, transcription | catalog cost 0 |
| `modhash-code` | `zai/glm-5.3-flash` | codec and algorithm crates | cheapest lane with a real price |
| `modhash-hard` | `zai/glm-5.3` | H.264, MP3, MP4, PDF | most expensive, used only where a silent bug is worst |
| `modhash-review` | `devin/swe-2` | adversarial verification | read-only by construction |

Costs below are what `omp models --json` reports, in dollars per million
tokens, at the time this file was written. They are catalog estimates and do
not include any plan or credit arrangement, so treat them as a ratio rather
than an invoice.

| model | context | max out | in | out |
|---|---|---|---|---|
| `openrouter/stealth/space-bunny-alpha` | 1,000,000 | 524,288 | 0 | 0 |
| `zai/glm-5.3-flash` | 1,000,000 | 131,072 | 0.15 | 0.50 |
| `devin/swe-2` | 262,000 | 128,000 | 0.75 | 3.75 |
| `zai/glm-5.3` | 1,000,000 | 131,072 | 1.40 | 4.40 |

**These numbers say nothing about quality.** No benchmark was run. What
decides whether a model is good enough for a lane is whether its crate passes
that crate's conformance vectors, which is what `modhash-review` checks. If a
lane keeps failing its vectors, swap the model - do not argue about it.

## Changing the routing

Edit the `model:` line and dispatch again. To route through a named role
instead of a literal selector, use a role alias and define it once:

```yaml
# .omp/config.yml
modelRoles:
  fast_worker: zai/glm-5.3-flash
  heavy_worker: zai/glm-5.3
```

```yaml
# .omp/agents/modhash-code.md
model: "@fast_worker"
```

That indirection is worth it once a model is being tuned: one edit retunes
every lane that points at the role.

## Spawning

Lanes do not spawn other lanes; `spawns` is not set, so each is a leaf. The
parent decides the wave structure, because only the parent can see the crate
dependency DAG.
