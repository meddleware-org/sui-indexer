# Security Policy

## Scope

The `sui-indexer` service source and image. Out of scope: the Sui full node, the client packages
(see their own policies), Cloudflare, and the cluster.

## Security model

1. **Display-only.** The indexer's data must never authorise anything. A consumer that trusts it
   for access decisions is misusing it; every row carries its transaction digest and checkpoint so
   it can be checked against a full node.
2. **No chain secrets.** It reads public chain data: no keys, no tokens. The only credentials are its two
   database logins (below), each in its own Secret; they are never logged and never put in an error, and the
   settings refuse an empty password.
3. **Read-only surface.** Only GET, HEAD and OPTIONS. Input is validated strictly, and SQL is
   parameterised. The API process connects as a `SELECT`-only PostgreSQL role (no write grant, `default_transaction_read_only`;
   every pooled connection also sets it) and cannot run DDL, whatever a compromised process tries; it has no chain
   access. Only the separate ingester writes the index, as the database owner, and it alone applies migrations.
   Exactly one writer is enforced by an advisory lock held for the life of its connection.
4. **Bounded cost.**
   - Per-IP token buckets on `CF-Connecting-IP`, behind the Cloudflare cache and rate-limit rule.
   - Request and header timeouts; at most 1024 open connections and a 32 KiB request head.
   - Page size limits.
5. **Least privilege.**
   - The image is distroless, runs as nonroot and has no shell. The services are Rust with **no `unsafe` code**.
   - The pods have a read-only root filesystem, all capabilities dropped and no writable volume at all (all
     state is in PostgreSQL). The ingester and the API are separate Deployments with separate database logins;
     NetworkPolicies admit only the ingress controller and the status probe to the API, and nothing to the
     ingester; the database namespace admits only these two pods, by label, on 5432.
   - **The database connection is authenticated both ways.** `sslmode=verify-full` against the tier's CA
     (mounted from a ConfigMap) is required; the other modes are refused. Each role can reach only its own
     database (`REVOKE ALL … FROM PUBLIC`), with a connection limit and statement timeouts.

**Shared fate.** The database tier also serves the auth stack; a runaway client is bounded by per-role limits and
timeouts, but an outage of the tier takes the index with it. The index is display data and its clients fall back to the
full node, so the effect is slower lists, not wrong or missing authorisation. Durability is the tier's backup.

`CF-Connecting-IP` is trusted because the service is reachable only through the Cloudflare tunnel.
If it is ever exposed directly, the header can be forged to dodge the per-IP limit.

## Reporting a vulnerability

Please **do not** open a public issue. Email **<security@meddleware.co.uk>** with a description,
reproduction steps, and the version or commit. Acknowledgement within **3 business days**; a plan
within **14 days** for confirmed issues.
