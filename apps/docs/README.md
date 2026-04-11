# @betterhook/docs

Mintlify documentation site for betterhook. Deploys to Mintlify.

## Local preview

```sh
# from the repo root
pnpm install
pnpm --filter @betterhook/docs run dev

# or directly
cd apps/docs
mint dev
```

Opens http://localhost:3000 with hot reload.

## Check links

```sh
pnpm --filter @betterhook/docs run check
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
