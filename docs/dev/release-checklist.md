# Release checklist: the `petal` CLI

The ordered commands for cutting a `petal` release, deploying the site that
serves the installer, and testing the one-liner on a clean machine.
[releasing.md](releasing.md) explains how the pieces fit together; this page is
the procedure. Garden is released separately, see
[releasing-garden.md](releasing-garden.md).

Nothing here has been run yet. Steps 4, 6 and 7 are the outward-facing ones (a
push, a tag, a deploy).

## Where things stand (checked 2026-10-09)

| Check | Result |
|-------|--------|
| `https://petal-lang.org/install.sh` | 404. The file is in the site repo (`frontend/public/install.sh`) but the deployed site predates it. |
| `https://petal-lang.org/uninstall.sh` | 404, same cause. |
| `https://petal-lang.org/download/` | 404, same cause. The home page, `/docs/` and `/examples/` return 200. |
| `github.com/andyfischer/petal/releases/latest/download/petal-<target>.tar.gz` | 404. The repo is public, but its only release is `garden-v0.1.0`, which "latest" resolves to. |
| `Release Petal` workflow | Never run (0 runs), so the two Linux musl builds are untested. |
| `CI` workflow on `main` | Failing in the "Run Vitest tests" step, on the last six pushes. |
| `petal --version` | `petal 0.1.0` |
| Installer vs. workflow | Match. `dist/install.sh` is byte-identical to the site's copy, and each of the four targets it can ask for is built and named the same by the workflow. Tested locally, see "What was dry-run" below. |

## Decisions to make first

1. **The version number.** DECIDED 2026-10-09: the next `0.x` number. No
   Petal release exists yet (the repo's only release is `garden-v0.1.0`), so
   the first one is `v0.1.0`, which `core/Cargo.toml` already says; step 2
   needs no version bump this time. The reasoning before the decision:
   `core/Cargo.toml` says `0.1.0`. The todo item is
   titled "nothing says 1.0", so the choice is between shipping `0.1.0` as it
   is, a `0.x` that signals more than a first draft, or `1.0.0`. A `1.0.0` is a
   stability promise that the README's status line contradicts (next item), and
   the todo list still has open items in the "silent wrong results" class.
   A pre-release tag such as `v1.0.0-rc.1` is also possible: the workflow
   publishes it as a pre-release and leaves "latest" alone, so the one-liner
   keeps installing the last full release.
2. **The README status line.** It reads "This project is in an early,
   experimental phase. Large backwards-incompatible changes are still
   happening. Stability not guaranteed." Keep it, soften it, or replace it,
   to agree with the version chosen above. The same decision applies to the
   site's home page copy.
3. **Whether to release with CI red.** The release workflow does not run the
   test suite, so a red `main` does not block it mechanically.
4. **Release notes.** The workflow uses GitHub's generated notes
   (`generate_release_notes: true`). With no earlier `v*` tag these will list
   every merged PR. The alternative is to paste the `CHANGELOG.md` section into
   the release after it is published (step 6).

## 1. Preflight

```bash
cd ~/petal
git status --short            # expect: clean
git fetch origin && git status -sb   # expect: main level with origin/main
df -h /                       # a release build needs a few GB free
```

Run the tests the release should stand on. The vitest suite is long, so bound
it (macOS has no `timeout`):

```bash
cargo test --manifest-path core/Cargo.toml
perl -e 'alarm shift; exec @ARGV' 1800 npx vitest run
npm run scan-secrets          # the history becomes more visible after a release
```

## 2. Set the version and the changelog

Replace `X.Y.Z` throughout with the version decided above.

```bash
cd ~/petal
sed -i '' 's/^version = "0.1.0"$/version = "X.Y.Z"/' core/Cargo.toml
git diff core/Cargo.toml      # expect: exactly one line, under [package]
```

Seven `Cargo.lock` files record petal's version, and both release workflows
build with `--locked`, so each has to be refreshed or the next `--locked` build
of that workspace fails (this includes Garden's):

```bash
for d in core core-libs/petal-ui garden integrations/petal-c-bridge \
         integrations/petal-desktop-sdl examples/custom-apps/petal-fantasy-nes \
         examples/custom-apps/petal-fps; do
  cargo metadata --format-version 1 --manifest-path "$d/Cargo.toml" >/dev/null
done
git status --short            # expect: core/Cargo.toml and the seven Cargo.lock files
cargo build --release --locked --manifest-path core/Cargo.toml
core/target/release/petal --version   # expect: petal X.Y.Z
```

In `CHANGELOG.md`, rename `## Unreleased` to `## X.Y.Z - YYYY-MM-DD`, add
anything merged since it was written, and put a new empty `## Unreleased`
above it. Apply the README decision. Then:

```bash
git add core/Cargo.toml CHANGELOG.md README.md \
  core/Cargo.lock core-libs/petal-ui/Cargo.lock garden/Cargo.lock \
  integrations/petal-c-bridge/Cargo.lock integrations/petal-desktop-sdl/Cargo.lock \
  examples/custom-apps/petal-fantasy-nes/Cargo.lock examples/custom-apps/petal-fps/Cargo.lock
git commit -m "chore(release): petal X.Y.Z"
```

## 3. Rehearse the installer locally

This packages the binary the way the workflow does and runs the real installer
against it through a `file://` release base. It publishes nothing and installs
into a scratch directory.

```bash
cd ~/petal
target=$(rustc -vV | sed -n 's/host: //p')
rel=$(mktemp -d)
mkdir -p "$rel/latest/download/petal-$target"
cp core/target/release/petal LICENSE README.md "$rel/latest/download/petal-$target/"
( cd "$rel/latest/download" && tar -czf "petal-$target.tar.gz" "petal-$target" \
  && shasum -a 256 "petal-$target.tar.gz" > "petal-$target.tar.gz.sha256" )

home=$(mktemp -d)
HOME="$home" SHELL=/bin/zsh PETAL_RELEASE_BASE="file://$rel" sh dist/install.sh
env -i HOME="$home" PATH="$home/.petal/bin:/usr/bin:/bin" petal --version      # petal X.Y.Z
env -i HOME="$home" PATH="$home/.petal/bin:/usr/bin:/bin" petal run -e 'print(1 + 2)'   # 3
cat "$home/.zshrc"            # expect: one "# petal" block
HOME="$home" sh "$home/.petal/uninstall.sh"
ls -a "$home"                 # expect: no .petal; .zshrc has no petal lines
```

## 4. Push and dry-run the release builds

The workflow has never run, so build all four targets before a tag exists.
`workflow_dispatch` builds and uploads artifacts but skips the publish job.

```bash
cd ~/petal
git push origin main
gh workflow run release-petal.yml --ref main
sleep 10 && gh run watch "$(gh run list --workflow release-petal.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Expect four green `Build <target>` jobs and a skipped `Publish GitHub Release`.
The `aarch64-apple-darwin` and `x86_64-unknown-linux-musl` jobs also run the
binary they built (`--version`, and a one-line program) before packaging it.
Fix any build failure on `main` and repeat this step. Do not tag until it is
green.

## 5. Check the site repo before deploying it

`dist/install.sh` and `dist/uninstall.sh` in this repo are the source of
truth, and the site serves copies. They are identical today; confirm that is
still true:

```bash
diff ~/petal/dist/install.sh   ~/biz/petal-lang.org/frontend/public/install.sh
diff ~/petal/dist/uninstall.sh ~/biz/petal-lang.org/frontend/public/uninstall.sh
```

If either differs, copy the `dist/` version over and commit it in the site
repo. The site also vendors a Petal WASM build, the example sketches and the
stdlib manifest. Refresh them so the site's docs and playground match the
release (the syntax has moved since the site was last built, for example
`pub`):

```bash
cd ~/biz/petal-lang.org
pnpm install
pnpm build:sdk
pnpm sync:examples
pnpm sync:stdlib
git status --short            # review, then commit what changed
```

Known problems in the site repo, found while preparing this and not fixed
(they are outside the petal repo):

- `scripts/sync-stdlib.ts` runs the extractor with `cwd: <petal>/ts`. That
  directory no longer has it; the script is now `tools/extract-stdlib.ts` from
  the petal repo root. The sync prints a warning and keeps the committed, stale
  manifest instead of failing.
- `frontend/src/components/Nav.tsx` links "source" to `https://github.com/`
  rather than `https://github.com/andyfischer/petal`.
- `frontend/out/` is a build from August. The deploy rebuilds it, so this only
  matters if someone serves `out/` by hand.

## 6. Tag and publish

The tag must equal the Cargo version with a `v` prefix. The workflow fails the
build if they disagree.

```bash
cd ~/petal
git tag vX.Y.Z
git push origin vX.Y.Z
gh run watch "$(gh run list --workflow release-petal.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Then confirm the release is the repo's "latest" and holds eight assets (a
tarball and a `.sha256` for each of the four targets):

```bash
gh release list --json tagName,isLatest,isPrerelease   # expect: vX.Y.Z has isLatest true
gh release view vX.Y.Z --json assets --jq '.assets[].name'
for t in aarch64-apple-darwin x86_64-apple-darwin \
         x86_64-unknown-linux-musl aarch64-unknown-linux-musl; do
  for ext in tar.gz tar.gz.sha256; do
    printf '%s ' "petal-$t.$ext"
    curl -sL -o /dev/null -w '%{http_code}\n' \
      "https://github.com/andyfischer/petal/releases/latest/download/petal-$t.$ext"
  done
done                          # expect: eight lines ending in 200
```

If `isLatest` is false, run `gh release edit vX.Y.Z --latest`. To replace the
generated notes with the changelog section, save that section to a file and run
`gh release edit vX.Y.Z --notes-file <file>`.

The installer can be tested against the release now, before the site is
deployed, by running the repo's copy:

```bash
home=$(mktemp -d)
HOME="$home" PETAL_NO_MODIFY_PATH=1 sh ~/petal/dist/install.sh
"$home/.petal/bin/petal" --version    # petal X.Y.Z
```

## 7. Deploy the site

```bash
cd ~/biz/petal-lang.org
deploy run deploy-frontend.qc
```

Then check what was 404:

```bash
for p in install.sh uninstall.sh download/ docs/ ; do
  printf '%s ' "$p"; curl -s -o /dev/null -w '%{http_code} %{content_type}\n' "https://petal-lang.org/$p"
done                          # expect: 200 on each
curl -fsSL https://petal-lang.org/install.sh   | diff - ~/petal/dist/install.sh     # expect: no output
curl -fsSL https://petal-lang.org/uninstall.sh | diff - ~/petal/dist/uninstall.sh   # expect: no output
npx tsx bin/smoke-test.ts
```

The site's 404 page is HTML served with status 404, so `curl -f` in the
one-liner fails cleanly rather than piping HTML into `sh`. Keep it that way.

## 8. Test the one-liner on clean machines

Linux, in throwaway containers (needs Docker on some machine). The first is a
glibc distro with curl, the second is musl with only busybox `wget`, which
takes the installer's other download path:

```bash
docker run --rm ubuntu:24.04 sh -c '
  apt-get update -qq && apt-get install -y -qq curl ca-certificates >/dev/null &&
  curl -fsSL https://petal-lang.org/install.sh | sh &&
  ~/.petal/bin/petal --version &&
  ~/.petal/bin/petal run -e "print(1 + 2)" &&
  sh ~/.petal/uninstall.sh && test ! -e ~/.petal'

docker run --rm alpine:3 sh -c '
  wget -qO- https://petal-lang.org/install.sh | sh &&
  ~/.petal/bin/petal --version &&
  ~/.petal/bin/petal run -e "print(1 + 2)"'
```

Add `--platform linux/arm64` or `--platform linux/amd64` to cover the
architecture the Docker host is not.

macOS, on a machine or user account that has never had Petal or a Rust
toolchain (a new user account in System Settings is enough). Open a new
terminal there and run exactly what the README says:

```bash
curl -fsSL https://petal-lang.org/install.sh | sh
```

Open a second new terminal, so the PATH edit is what finds the binary, and:

```bash
which petal                   # ~/.petal/bin/petal
petal --version               # petal X.Y.Z
printf 'fn sq(x)\n  x * x\nend\nprint([1, 2, 3] |> map(sq))\n' > hello.ptl
petal run hello.ptl           # [1, 4, 9]
curl -fsSL https://petal-lang.org/uninstall.sh | sh
```

Do this on Apple Silicon and, if one is available, on an Intel Mac: the
`x86_64-apple-darwin` binary is cross-compiled and is the one target that
nothing has executed by this point.

Pinning works the same way and is worth one run:

```bash
curl -fsSL https://petal-lang.org/install.sh | PETAL_VERSION=vX.Y.Z sh
```

## 9. Afterwards

- Append `DONE` under item 1 in [../tasks/todo.md](../tasks/todo.md) and
  remove §1.1 and §1.4 from the bug list.
- The next Garden release needs nothing special: it publishes with
  `make_latest: false`, so it cannot take the "latest" slot back.

## What was dry-run while writing this

Done locally on macOS arm64 with a release build, publishing nothing:

- The workflow's own "Package tarball + checksum" script, extracted from the
  YAML and run for each of the four matrix targets, produces
  `petal-<target>.tar.gz` (containing `petal-<target>/petal`) and
  `petal-<target>.tar.gz.sha256`.
- `dist/install.sh`, with `uname` stubbed, asks for exactly those names for
  macOS arm64 and x86_64 and Linux x86_64/amd64 and aarch64/arm64, under both
  `latest/download/` and `download/<tag>/`, verifies the checksum, and
  installs. An unsupported architecture exits 1 with a message.
- A wrong checksum aborts the install.
- The PATH edit, a run of the installed binary with an otherwise empty
  environment and from an unrelated directory, and the uninstaller all work.
- The tag-versus-Cargo-version check and the smoke-test commands added to the
  workflow were run as shell locally. The workflow file itself was only parsed,
  not executed; step 4 is its first real run.

Not verified: the Linux musl builds (no musl Rust targets installed locally),
the `x86_64-apple-darwin` cross build, and the site deploy.

One weakness seen and left alone: on macOS, `sha256sum -c` exits 0 when the
`.sha256` file has no well-formed line (it only warns), so the installer
reports "checksum verified" for a malformed checksum file. A checksum file with
a wrong hash is rejected correctly. Fixing it means comparing the two hashes in
the script instead of relying on `-c`, in both `dist/install.sh` and the site's
copy.
