# Security Policy

## Scope

The `sui-indexer` service source and image. Out of scope: the Sui full node, the client packages
(see their own policies), Cloudflare, and the cluster.

## Security model

1. **Display-only.** The indexer's data must never authorise anything. A consumer that trusts it
   for access decisions is misusing it; every row carries its transaction digest and checkpoint so
   it can be checked against a full node.
2. **No secrets.** It reads public chain data. There are no keys, tokens or credentials.
3. **Read-only surface.** Only GET, HEAD and OPTIONS. Input is validated strictly, and SQL is
   parameterised.
4. **Bounded cost.**
   - Per-IP token buckets on `CF-Connecting-IP`, behind the Cloudflare cache and rate-limit rule.
   - Request and header timeouts.
   - Page size limits.
5. **Least privilege.**
   - The image is distroless, runs as nonroot and has no shell.
   - The pod has a read-only root filesystem and all capabilities dropped. Only the data volume is
     writable.

`CF-Connecting-IP` is trusted because the service is reachable only through the Cloudflare tunnel.
If it is ever exposed directly, the header can be forged to dodge the per-IP limit.

## Reporting a vulnerability

Please **do not** open a public issue. Email **<security@meddleware.co.uk>** with a description,
reproduction steps, and the version or commit. Acknowledgement within **3 business days**; a plan
within **14 days** for confirmed issues.
