---
name: release
description: Cut a NymPlayer release - bump the version, date the changelog, refresh the readme screenshot, run CI on all three platforms, then tag vX.Y.Z so the Release workflow publishes the downloads.
argument-hint: "[major|minor|patch|X.Y.Z]"
disable-model-invocation: true
---

# Release NymPlayer

Pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml`. That workflow
builds and tests on Windows, macOS and Linux, then publishes a GitHub release
with all three downloads. It fails if the tag doesn't match the version in
`Cargo.toml`, or if any platform fails. Releases are commits on `master`.

Steps 1–5 are local and can be undone. **Step 6 publishes: confirm with the
user before pushing.**

## 1. Pick the version

- Read `## [Unreleased]` in `CHANGELOG.md`. If it's empty, there's nothing to
  release: say so and stop.
- If `$ARGUMENTS` gives the version, use it. Otherwise follow
  [Semantic Versioning](https://semver.org/) while the app is still 0.x:
  - New features or a behaviour change: minor (`0.4.0` to `0.5.0`).
  - Only fixes: patch (`0.4.0` to `0.4.1`).
- Check that the working tree is clean apart from the release itself, and that
  you're on `master` or a branch that will be merged into it first.

## 2. Bump and date

- **`Cargo.toml`:** set `version = "X.Y.Z"`. The next `cargo build` updates
  `Cargo.lock`, which goes in the same commit.
- **`CHANGELOG.md`:**
  - Under `## [Unreleased]`, insert `## [X.Y.Z] - YYYY-MM-DD` with today's
    date, leaving `[Unreleased]` empty above it.
  - At the bottom, point `[Unreleased]` at `compare/vX.Y.Z...HEAD`.
  - Add `[X.Y.Z]: https://github.com/nymankla/NymPlayer/compare/v<previous>...vX.Y.Z`.
- Read the entries back as a user would and make sure each one matches what
  the app now does.

## 3. Refresh the readme screenshot (if the window changed)

`assets/screenshot.png` is the whole window, title bar included, at 879×675.
Retake it whenever the release changes anything visible. Use the
`run-nymplayer` driver. It plays "All PC sound", so there's no mic feedback:

```powershell
cargo build --release
$d = '.claude/skills/run-nymplayer/driver.ps1'
powershell -ExecutionPolicy Bypass -File $d launch -Unmuted -Input pcsound -Volume 0.84
powershell -ExecutionPolicy Bypass -File $d ss readme -Frame
powershell -ExecutionPolicy Bypass -File $d quit
Copy-Item "$env:TEMP\nymplayer-run\readme.png" assets\screenshot.png
```

- Look at the image before using it.
- If the window's layout changed, update the `alt` text in `readme.md`.

## 4. Check locally

```powershell
cargo fmt --check
cargo clippy --all-targets   # must be warning-free: CI and release builds use RUSTFLAGS=-D warnings
cargo test
cargo build --profile dist   # the published build; note the exe size for the changelog if it moved
```

## 5. Commit

- Commit everything with a subject like `<headline changes>; vX.Y.Z`, as in
  `Recording, All PC sound, smaller and leaner; v0.4.0`.
- In the body, write one bullet per change. Add `Closes #N` for issues it
  fixes.

## 6. CI, then tag (confirm first)

- Ask the user before going on. Pushing `master` and the tag is public, and the
  tag publishes downloads.
- Then push the commit and run CI on it. CI only runs when started by hand:

```powershell
git push origin master
gh workflow run ci.yml --ref master
gh run list --workflow ci.yml --limit 1        # get the run ID
gh run watch <id> --exit-status
```

- **CI is green:** tag the release, push the tag, and watch the Release run.

  ```powershell
  git tag vX.Y.Z
  git push origin vX.Y.Z
  gh run list --workflow release.yml --limit 1
  gh run watch <id> --exit-status
  ```

- **CI is red:** don't tag. Fix the problem, commit, and run CI again. macOS
  and Linux failures usually come from code outside `src/system/`; see
  CLAUDE.md, "Platform code".

When the Release run finishes, check that the release has all three
downloads, then give the user the link:

```powershell
gh release view vX.Y.Z
```

## If a release run fails after the tag is pushed

- Nothing is published until every platform passes.
- Fix the problem on `master`.
- Move the tag to the fixed commit, but only after the user agrees, since this
  rewrites a public tag:

  ```powershell
  git push origin :refs/tags/vX.Y.Z
  git tag -f vX.Y.Z
  git push origin vX.Y.Z
  ```
