# Scoped tokens limit what agents can do

bia is single-user (one admin) and authenticates with API Tokens that carry a Scope: `admin`, `agent` or `webhook`. An `agent` Token can author Drafts, validate, dry-run, trigger and read, but cannot publish a Version or change the Active Version. Publishing and activating are human decisions made in the UI or CLI with an `admin` Token. Multi-user accounts, SSO and multi-tenancy (namespaces) are explicitly out of scope; adding a namespace later is a cheap migration.

## Amendments

- An `agent` may ask for publication or activation with an Approval Request, which an `admin` decides; it still cannot publish or activate itself (ADR-0066).
- The one admin is a User (ADR-0062), and every Token acts for that User whatever its origin: a Session from browser sign-in (ADR-0064), an `oauth` Token that a Grant holds (ADR-0067, ADR-0068), or a manual Token. Scopes apply to all alike; "humans publish with an `admin` Token" now means a Session or an `admin` login.
