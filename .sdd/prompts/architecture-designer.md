# Architecture Designer Prompt

You are creating an architecture design from an approved PRD or imported requirements document. (Full-tier node: on the light tier the change spec's `## Design Notes` section carries the design decisions instead of a separate architecture.md.)

Inputs:

- `prd.md`
- relevant current codebase files
- `.sdd/memory/principles.md`
- `.sdd/specs/current/` where relevant
- `docs/authoring-discipline.md` (architecture authoring rules — binding)

Design vocabulary (use these terms exactly in `architecture.md`):

- **Module** — anything with an interface and an implementation; scale-agnostic (function, class, package, tier-spanning slice).
- **Interface** — everything a caller must know to use the module correctly: signature plus invariants, ordering constraints, error modes, required configuration, performance characteristics.
- **Seam** — the place where a module's interface lives; where behaviour can be altered without editing in that place. Seam placement is its own design decision, distinct from what goes behind it.
- **Adapter** — a concrete thing that satisfies an interface at a seam.
- **Depth** — leverage at the interface: behaviour exercised per unit of interface a caller must learn. Prefer deep modules (small interface, substantial implementation) over shallow ones (interface nearly as complex as the implementation).

Design rules:

- The interface is the test surface: callers and tests cross the same seam. A module whose tests need to reach past its interface is the wrong shape.
- One adapter means a hypothetical seam; two adapters means a real one. Introduce a port only when at least two adapters are justified (typically production + test).
- Classify each module's dependencies to decide its testing seam: in-process (test directly), local-substitutable (test against the stand-in, seam stays internal), remote-but-owned (port at the seam, in-memory adapter for tests), true external (injected port, mock adapter in tests).

Process:

1. Identify the current architecture and seams.
2. Design it twice: for each meaningful decision, present 2-3 genuinely different approaches — for module interfaces, differentiate by design pressure (minimal interface / maximal flexibility / trivial default case for the most common caller). Compare on depth, locality (where change concentrates), and seam placement.
3. Recommend one approach — or a hybrid — with trade-offs.
4. When discussing with the user, follow `docs/discussion-protocol.md`: one decision at a time, dependency order, recommendation first.
5. Draft `architecture.md` using `templates/architecture.md`. In the module table, state each interface with its invariants, error modes, and constraints, not just method names.
6. Keep implementation steps out of the architecture unless they clarify a decision.

Output:

- architecture path
- key decisions
- risks
- open questions
- recommended next step
