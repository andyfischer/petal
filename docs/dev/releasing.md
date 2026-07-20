# Releasing Petal

This describes how prebuilt `petal` binaries are built and published, and how
the one-line installer works end to end.

## The pieces

| Piece | Where | Role |
|-------|-------|------|
| `.github/workflows/release.yml` | this repo | Builds the CLI for every target on a tag push and publishes a GitHub Release. |
| `dist/install.sh` | this repo | The installer. Served at `https://petal-lang.org/install.sh`. |
| `dist/uninstall.sh` | this repo | The uninstaller. Served at `https://petal-lang.org/uninstall.sh`. |
| `frontend/public/install.sh` / `uninstall.sh` | `petal-lang.org` repo | The **served copies** — mirror `dist/` (see "Keeping the scripts in sync"). |

End-to-end flow:

```
user runs curl … petal-lang.org/install.sh | sh
      │
      ▼
install.sh detects OS/arch → downloads
  github.com/andyfischer/petal/releases/latest/download/petal-<target>.tar.gz
      │  (verifies the .sha256 alongside it)
      ▼
extracts, installs to ~/.petal/bin/petal, adds it to PATH
```

## Cutting a release

1. Bump the version in `rust/Cargo.toml` (`version = "x.y.z"`) and commit.
2. Tag and push:

   ```bash
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. The `Release` workflow runs. Its `build` matrix compiles the `petal` binary
   for four targets and uploads each as an artifact:

   | Target | Runner | How |
   |--------|--------|-----|
   | `aarch64-apple-darwin` | `macos-14` | native `cargo build --release` |
   | `x86_64-apple-darwin` | `macos-14` | cross via `cargo build --target` |
   | `x86_64-unknown-linux-musl` | `ubuntu-latest` | `cargo-zigbuild` (static) |
   | `aarch64-unknown-linux-musl` | `ubuntu-latest` | `cargo-zigbuild` (static) |

   Each build is packaged as `petal-<target>.tar.gz` (containing
   `petal-<target>/petal`) plus a `.tar.gz.sha256` checksum.

4. The `release` job downloads all artifacts and publishes them to a GitHub
   Release for the tag (with auto-generated notes). Because the asset names are
   version-independent, `releases/latest/download/petal-<target>.tar.gz`
   always resolves to the newest release — which is exactly what the installer
   requests when no `PETAL_VERSION` is set.

You can trigger the workflow from the Actions tab (`workflow_dispatch`) to
**dry-run the builds** without publishing — the `release` job only runs on a
`v*` tag push.

## Installer environment variables

`dist/install.sh` honors:

- `PETAL_INSTALL` — install prefix (default `$HOME/.petal`; binary at `$PETAL_INSTALL/bin/petal`).
- `PETAL_VERSION` — pin a specific tag, e.g. `v0.1.0` (default: latest release).
- `PETAL_RELEASE_BASE` — override the releases base URL (used for local testing).
- `PETAL_NO_MODIFY_PATH=1` — install the binary but don't edit any shell rc file.

## Testing the installer locally

You can exercise the whole download → verify → install → uninstall path against
a locally-built binary, without publishing anything, using a `file://` release
base:

```bash
# 1. build + package like the workflow does
cargo build --release --manifest-path rust/Cargo.toml
target=$(rustc -vV | sed -n 's/host: //p')
mkdir -p /tmp/rel/download/v0.0.0/pkg/petal-$target
cp rust/target/release/petal /tmp/rel/download/v0.0.0/pkg/petal-$target/petal
( cd /tmp/rel/download/v0.0.0/pkg && tar -czf ../petal-$target.tar.gz petal-$target )
( cd /tmp/rel/download/v0.0.0 && shasum -a 256 petal-$target.tar.gz > petal-$target.tar.gz.sha256 )

# 2. install into a scratch prefix
PETAL_RELEASE_BASE="file:///tmp/rel" PETAL_VERSION=v0.0.0 \
  PETAL_INSTALL=/tmp/petal-test PETAL_NO_MODIFY_PATH=1 sh dist/install.sh

/tmp/petal-test/bin/petal --version

# 3. uninstall
PETAL_INSTALL=/tmp/petal-test sh /tmp/petal-test/uninstall.sh
```

## Keeping the scripts in sync

`dist/install.sh` and `dist/uninstall.sh` are the source of truth. The website
serves copies at `frontend/public/`. After editing either script here, copy it
into the website repo and redeploy:

```bash
cp dist/install.sh   ../petal-lang.org/frontend/public/install.sh
cp dist/uninstall.sh ../petal-lang.org/frontend/public/uninstall.sh
# then, in the petal-lang.org repo:  deploy run deploy-frontend.qc
```

(The uninstaller embedded inside `install.sh` — dropped at `~/.petal/uninstall.sh`
— is a trimmed copy of `dist/uninstall.sh`; keep it aligned when changing
uninstall behavior.)
