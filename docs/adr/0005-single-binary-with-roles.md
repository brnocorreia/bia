# One binary with roles, correct under multiple replicas from day one

bia ships as one binary, `bia`. `bia serve --role api|worker|scheduler|all` decides what a process does, and the other subcommands are clients. Development runs `--role all`. Every coordination path (Job claims, Leases, Schedule firing) is written to stay correct with N replicas of any Role from the first commit. With Postgres primitives this costs almost nothing, while retrofitting it later would be expensive. Production hardening of multi-replica deployments is a later milestone, but correctness is not deferred.
