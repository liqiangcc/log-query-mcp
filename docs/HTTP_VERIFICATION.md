# HTTP Verification

## Purpose

HTTP verification proves the behavior of a running `log-query-mcp` instance from the external Streamable HTTP boundary. It is intentionally separate from `./scripts/verify all`, which only verifies source/build/contracts and does not require a deployed service.

## Test assets

Executable HTTP cases live under:

```text
tests/http/
```

The first smoke case is:

```text
tests/http/mcp/initialize.http
```

It sends the same MCP `initialize` operation documented in the README and asserts the externally observable response.

## Run

Install httpYac:

```bash
npm install -g httpyac
```

Point verification at a running instance. `BASE_URL` does not include `/mcp`:

```bash
BASE_URL=http://127.0.0.1:8000 ./scripts/verify http-smoke
```

The same command can target staging after deployment:

```bash
BASE_URL=https://staging.example.internal ./scripts/verify http-smoke
```

## Separation of concerns

```text
./scripts/verify all
→ source/build/contract verification
→ no running service required

./scripts/verify http-smoke
→ real HTTP boundary verification
→ running service + BASE_URL + httpYac required
```

The HTTP case is tagged `smoke`, `deployment`, and `production-safe` because MCP initialization is read-only and does not modify log data.

## Bug regression

When an HTTP-visible bug is fixed, add the smallest `.http` case that asserts the correct behavior. Keep the same case after the fix:

```text
Before fix: FAIL
After fix: PASS
After deployment: PASS
```

Do not delete the case after the incident is closed; it becomes a permanent regression asset.

## Current pilot status

The asset and stable command are implemented on the verification pilot branch. Runtime execution remains `NOT VERIFIED` until a reachable service and an execution environment with httpYac are available. GitHub-hosted Actions are currently blocked at the account/billing layer before any workflow step starts, so that condition is classified as `TEST_ENVIRONMENT_FAILURE`, not an HTTP or code failure.
