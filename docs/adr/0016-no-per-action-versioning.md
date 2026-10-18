# Actions are not individually versioned

Action names carry no version (`http.request`, not `http.request@1`). Breaking changes go through the DSL's `schema_version` and its migrations. During 0.x, changes to an Action's param schema must be additive. Versioning each Action would multiply the compatibility surface for little benefit in a single-maintainer project.
