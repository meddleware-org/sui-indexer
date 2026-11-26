# Contributing to sui-indexer

Thank you for looking. sui-indexer is part of [Meddleware](https://meddleware.co.uk): open tools you can use, take everything when you leave,
and run yourself. This file says how to build it, test it and propose a change.

## Build and test

```bash
DOCKER=podman bash scripts/test-postgres.sh start && . "${TMPDIR:-/tmp}/sui-indexer-test-pg/env.sh"   # a throwaway PostgreSQL 18 with TLS; sets TEST_DATABASE_URL
cargo fmt --check                                                     # formatting
cargo clippy --workspace --all-targets --locked -- -D warnings        # lints; missing docs on any item, private ones included, fail the build
cargo test --workspace --locked                                       # every test gets its own database and the two production roles
cargo deny check                                                      # advisories, licences, bans, sources
bash scripts/smoke.sh <image>                                         # ingester and API in containers against PostgreSQL
DOCKER=podman bash scripts/test-postgres.sh stop
```

CI runs the same commands on every push and pull request (`.github/workflows/`). A change is ready when they all pass locally.

## Proposing a change

1. Open an issue first for anything larger than a fix, so the design can be agreed before the work.
2. Keep a change to one purpose. Write a short subject line in the imperative ("Fix the relay timeout"), and say why in the pull request.
3. Add or update tests with the behaviour they cover. A bug fix comes with a test that failed before it.
4. Add a line to `CHANGELOG.md` under `Unreleased`.
5. Security problems go to the address in [`SECURITY.md`](SECURITY.md), never to a public issue.

## House rules

- **Licence.** Everything here is [0BSD](LICENSE). By contributing you agree your contribution is released under it.
- **Versions.** Until the platform's mainnet launch, every release is a patch bump, breaking changes included; the changelog says what broke.
- **No secrets, no personal data** in code, tests, fixtures, comments or commit messages.
- **Privacy.** Nothing is stored in a visitor's browser before they act, and nothing is sent to a host the project does not document. No telemetry, analytics or tracking.
- **Open standards.** Where a standard exists and fits, use it; the standards inventory in [the audit](docs/audit/sui-indexer-audit.md) (B.RFC-1) lists the ones this project speaks.

## Documentation

Every file explains itself, line by line where the code is not self-evident:

- a header on every file saying what it is for;
- a doc comment on every declaration (TSDoc, `///` in Rust and Move, Go doc comments), private ones included;
- comments that say **why**, cite the decision, issue or standard behind a choice, and are updated with the code;
- no `TODO` or `FIXME` without an issue or follow-up reference on the same line;
- every command a document shows has been run, and every link resolves.
