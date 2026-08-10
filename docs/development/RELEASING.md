# Release checklist

Before publishing a development release:

1. update `CHANGELOG.md`, `STATUS.md`, and any affected normative draft;
2. run `./scripts/dev-check.sh`;
3. run `./scripts/build-site.sh` and inspect the generated site;
4. regenerate conformance vectors when decoded semantics or syntax changed;
5. verify the browser demo over HTTP Range requests;
6. ensure no generated toolchain, target directory, secrets, or large private benchmark media are
   tracked;
7. tag only after the source archive rebuilds from a clean directory.

Stable format releases require a separate freeze review and are intentionally out of scope while the
project remains `0.x`.
