# Releasing Garden

Garden ships as a Homebrew formula in [facetlayer/homebrew-tap][tap]:

```bash
brew install facetlayer/tap/garden
```

This describes how those bottles are built and how the tap gets updated.
For the `petal` CLI's own release flow (tarballs + the `install.sh` installer),
see [releasing.md](releasing.md).

## Tag prefixes

This repo publishes more than one package, so releases are namespaced by tag:

| Tag pattern | Package | Workflow |
|-------------|---------|----------|
| `v*` | the `petal` CLI | `.github/workflows/release-petal.yml` |
| `garden-v*` | Garden | `.github/workflows/release-garden.yml` |

Garden releases are published with `make_latest: false`. The petal installer
resolves `releases/latest/download/petal-<target>.tar.gz`, so a Garden release
that claimed the "latest" slot would break `curl … petal-lang.org/install.sh`.

## The pieces

| Piece | Role |
|-------|------|
| `.github/workflows/release-garden.yml` | Builds Garden on a `garden-v*` tag, publishes the GitHub Release, then updates the tap. |
| `.github/scripts/update-homebrew-tap.sh` | Generates one formula from a release's `SHA256SUMS` and pushes it. Shared by every package in this repo. |
| `facetlayer/homebrew-tap` | The public tap. `Formula/garden.rb` is generated — never edit it by hand. |

## Cutting a release

1. Bump `version` under `[workspace.package]` in `garden/Cargo.toml` and commit.
   The workflow refuses to build if the tag and that version disagree, because
   the formula's `test do` asserts the tag's version against `garden --version`.
2. Tag and push:

   ```bash
   git tag garden-v0.2.0
   git push origin garden-v0.2.0
   ```

3. The `build` matrix compiles for `aarch64-apple-darwin` (native on the
   `macos-14` runner) and `x86_64-apple-darwin` (cross-compiled from the same
   runner — Apple's clang and SDK target both architectures). Garden is a
   wgpu/winit GPU app; **macOS is the only supported platform**, so there are no
   Linux builds.
4. Each target is packaged as `garden-<target>.tar.gz` plus a
   `.tar.gz.sha256`, and published to a GitHub Release for the tag.
5. The `homebrew` job downloads the checksums, regenerates
   `Formula/garden.rb`, and pushes it to the tap.

Running the workflow from the Actions tab (`workflow_dispatch`) **dry-runs the
builds** — the `release` and `homebrew` jobs only run on a `garden-v*` tag push.

## What ships in the tarball

`garden` plus every builtin GPP client it spawns: `directory-browser`,
`git-log`, `garden-diff`, `main-menu`, `sqlite-browser`. `garden` resolves those
next to its own executable, so they must land in the same prefix — a missing one
breaks the matching `garden <app>` subcommand at runtime with no build error.
The list lives in `GARDEN_BINS` in the workflow; keep it in sync with
`BUILTIN_GPP_APPS` in `garden/garden-app/src/lib.rs`.

Everything else Garden needs (the `.ptl` drawers, the prelude, the bundled
SQLite) is compiled in, so the tarball is self-contained. Garden's config and
state directory is created on demand; the formula's caveats point at
`garden setup initialize-config-if-missing`.

## The tap token

The `homebrew` job needs a PAT with `contents: write` on
`facetlayer/homebrew-tap`, stored on this repo as `HOMEBREW_TAP_TOKEN`
(`FACETLAYER_PACKAGES_TOKEN` is accepted as a fallback). If neither secret is
set the job logs a notice and skips — the GitHub Release still publishes.

## Adding another package to the tap

`update-homebrew-tap.sh` is package-agnostic. Give the new package its own
`release-<name>.yml` with its own tag prefix, and add a job that checks the tap
out at `./tap`, writes the release's checksums to `./SHA256SUMS`, and calls the
script with `FORMULA` / `CLASS` / `DESC` / `HOMEPAGE` / `VERSION` / `TAG` /
`REPO` / `ASSET_PREFIX` / `BINS` / `TEST_CMD` (and optionally `CAVEATS`) set.
A target appears in the formula only when its checksum is in `SHA256SUMS`, so a
package that also ships Linux builds picks those up with no change to the script.

[tap]: https://github.com/facetlayer/homebrew-tap
