# Publishing Archi as open source on GitHub

Checklist for the first public release of this repository.

## Before the first push

1. **License** — `LICENSE` is MIT with copyright **IKER**.
2. **Remote** — `https://github.com/IkEr0228/Archi.git` (already in README / package metadata).
3. **Security contact** — Enable GitHub **Private vulnerability reporting** on the repo (Settings → Security).
4. **Secrets scan** — Confirm no API keys, passwords, or private paths in history:
   ```powershell
   git log -p --all -S "api_key" -- . | Select-Object -First 5
   ```
5. **Working tree** — Clean of `node_modules`, `target`, `build`, and other gitignored paths.

## Create / push the remote

On GitHub: **New repository** `IkEr0228/Archi` → public → **do not** initialize with README/LICENSE (already in tree).

```powershell
# From the Archi repo root
git remote add origin https://github.com/IkEr0228/Archi.git
# or if origin already exists:
git remote set-url origin https://github.com/IkEr0228/Archi.git
git branch -M master
git push -u origin master
```

If you prefer `main`:

```powershell
git branch -M main
git push -u origin main
```

CI workflow already listens to both `master` and `main`.

## After first push

1. Settings → General → Features: Issues, Discussions (optional)
2. Settings → Security → Code security: enable Dependabot alerts if desired
3. Settings → Secrets: none required for current CI
4. Create a **Release** when you ship a tagged build:
   ```powershell
   git tag -a v0.1.0 -m "Archi 0.1.0"
   git push origin v0.1.0
   ```
   Attach NSIS installer / EXE from `npm run tauri build` as release assets (binaries stay out of git).

## What stays out of git

| Path | Why |
| --- | --- |
| `node_modules/`, `target/`, `build/` | Build products |
| `benchmark-results/`, generated archives | Local fixtures |
| `*.log`, local scratch under `.gitignore` | Not product source |

## CI

`.github/workflows/ci.yml` runs on Windows: frontend tests, `svelte-check`, production frontend build, `rustfmt`, `cargo test`.

## Optional later

- GitHub Pages for docs
- Code signing for the NSIS installer
- `CODE_OF_CONDUCT.md` if the community grows
- Fill `repository` URL in `package.json` / `Cargo.toml` once the remote is final
