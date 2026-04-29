# betterhook

A git hooks manager that actually works with worktrees.

This is a **distribution package** that downloads the platform-specific
native binary from GitHub Releases on `npm install`. The binary is a
single static Rust executable with ~30 ms cold start.

For documentation, source code, and issues:
[github.com/leonardomso/betterhook](https://github.com/leonardomso/betterhook)

## Install

```sh
npm install -g betterhook
# or
npx betterhook init
```

## Supported platforms

- macOS arm64 (Apple Silicon)
- macOS x64 (Intel)
- Linux x64
- Linux arm64
