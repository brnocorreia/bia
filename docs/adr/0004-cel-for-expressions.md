# CEL is the only expression language

Expressions are CEL: typed, side-effect free, deterministic, with bounded evaluation cost. Fields that *are* conditions (`when`, `until`) take raw CEL. Params use Templates (`{{ expr }}`), and a Template that is exactly one expression keeps the expression's type. Text templating (minijinja) and jq-style transforms are not part of the Expression language. If they arrive, they arrive as Actions (`transform.template`), never as a second syntax inside Definitions.
