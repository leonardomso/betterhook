# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.0.x   | Yes       |

Betterhook is pre-release software. Security patches are applied to the
latest 0.0.x release only.

## Reporting a Vulnerability

**Do not open a public issue.** Instead, email **leonardo@maldonado.dev**
with:

1. A description of the vulnerability.
2. Steps to reproduce.
3. The impact you've assessed.

You'll receive an acknowledgement within 48 hours. We aim to publish a
fix within 7 days for confirmed issues.

## Scope

Betterhook executes user-configured shell commands via `sh -c`. The
security boundary is the same as the user's shell — betterhook does not
sandbox the commands it runs. If a malicious `betterhook.toml` is
checked into a repo, any command in it will run with the committer's
privileges. This is by design and consistent with every other git hooks
manager (husky, lefthook, pre-commit, hk).

Vulnerabilities we care about:

- Command injection via config field values that bypass the intended
  `sh -c` invocation (e.g., a glob pattern that escapes into a shell
  command).
- Path traversal in the cache, lock, or wrapper-install paths.
- Denial of service via crafted config files that cause unbounded
  memory or CPU usage in the parser or DAG resolver.
- Privilege escalation via the daemon's Unix socket.

Vulnerabilities we consider out of scope:

- Arbitrary code execution via a malicious `run = "..."` field. That's
  the intended behavior of a hooks manager.
