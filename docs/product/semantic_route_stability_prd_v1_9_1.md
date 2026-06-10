# Product Note: Semantic Route Stability v1.9.1

Players should not experience the same sentence as an object action once, a combat action once, and free narration once. v1.9.1 stabilizes the table experience by letting LLM semantic tools suggest meaning while Rust applies deterministic route rules.

The player-facing product goal is simple: if a player says they disarm an enemy, it reliably becomes an object interaction; if they ask whether a cable can be safely cut, it reliably becomes a technical check; if they are under a required reaction window, they must answer it unless they are clearly exiting, surrendering, or ending the conflict.
