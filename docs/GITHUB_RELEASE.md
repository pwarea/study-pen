# Preparing a release

The source ZIP is a clean copy of the files intended for GitHub. Extract it and review the contents before publishing. Keep the existing Cargo lockfile, license, and GitHub workflows.

## Package on Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts/build.ps1
```

The script runs tests and writes the portable ZIP, source ZIP, and archive checksums to `dist/`. Add `-SourceOnly` to package just the source. The portable package includes the executable, documentation, and license notices. The build strips debug symbols and remaps local build paths. Both archives use fixed entry timestamps.

## Publish from GitHub

1. Review the source snapshot and the public repository address used by the README download links and badge.
2. Push the approved source to a new repository. For the first release, tag the approved commit `v0.1.0`.
3. Download the assets from the **Release** workflow run for the tag and verify their checksums. Create the release from your GitHub account, attach the two ZIP files and checksum file, and add the release notes.

The tag must match the version in `Cargo.toml`. The workflow builds with the x86-64 MSVC toolchain and uses the public repository address for update checks. Pass `-GitHubRepository owner/repository` when building locally for a public repository. Local builds without that option have no configured update feed. Drafts do not appear in the feed.

Keep the portable asset name **study-pen-windows-x86_64-portable.zip** across releases. The README counter adds downloads of that asset across published releases. The source ZIP and checksum file include the version in their names.

For subsequent releases, bump the package version in `Cargo.toml`, refresh the `study-pen` entry in `Cargo.lock`, update `docs/RELEASE_NOTES.md`, and repeat the tag and release steps. You can also run the **Release** workflow manually to build packages for review.
