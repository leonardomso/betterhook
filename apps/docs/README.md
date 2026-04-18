# @betterhook/docs

Mintlify documentation site for betterhook. Deploys to Mintlify.

## Local preview

```sh
cd apps/docs
bun install
bun run dev
```

Opens http://localhost:3000 with hot reload.

## Check links

```sh
cd apps/docs
bun run check
```

## Structure

- `docs.json` — Mintlify config (theme, navigation, colors, logo, icons)
- `*.mdx` — pages (every path in `docs.json` → one MDX file)
- `logo/` + `favicon.svg` — brand assets
- `images/` — static assets referenced from MDX

## Deploying

Mintlify deploys automatically on push to the main branch once the
GitHub app is connected to this repo. There's no local `build` step —
the dev server is the preview.

See [Mintlify docs](https://mintlify.com/docs) for the full config
reference.
