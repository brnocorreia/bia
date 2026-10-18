# Scoped tokens limit what agents can do

bia is single-user (one admin) and authenticates with API Tokens that carry a Scope: `admin`, `agent` or `webhook`. An `agent` Token can author Drafts, validate, dry-run, trigger and read, but cannot publish a Version or change the Active Version. Publishing and activating are human decisions made in the UI or CLI with an `admin` Token. Multi-user accounts, SSO and multi-tenancy (namespaces) are explicitly out of scope; adding a namespace later is a cheap migration.
