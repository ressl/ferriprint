# Public source and repository workflow

Ferriprint's public development branch is `public-development` at
<https://github.com/ressl/ferriprint>. The canonical GitLab repository remains
private; its corresponding `public-development` branch and version tags contain
the **same public commits**, not a merge of the old private pilot history.

The initial public branch is a new root commit of the exact reviewed sanitized
source tree. The former private `main` history is preserved, not rewritten,
force-pushed or deleted. The private operator record identifies its predecessor
commit and shared tree hash. Those private historical deployment/network records
are deliberately not imported into the public commit graph or source archive.

## One-time clean export

1. Freeze the reviewed source and finish documentation, package inputs and notices.
   Keep its private predecessor/tree proof in the private operator record only.
2. Export that exact tree into an empty owned staging directory and initialize a
   **fresh independent Git object database** there. Do not use a linked worktree,
   shared clone, alternates or a copied private `.git`. Stage the reviewed exported
   files, including intentionally tracked files that an ignore rule could hide,
   and require the resulting tree ID to equal the reviewed private tree.
3. Create `public-development` as a root commit with **no parents**. Review its
   complete files, licenses and reachable history before building or publishing.
   The public object database must never contain private historical objects.
4. Build and exercise from this exact public commit. After the required review
   gates, push only its clean branch/tag from that independent repository to the
   intended GitHub destination and the existing private GitLab project.
5. Fetch the clean branch into the normal source checkout, switch future work to
   it without discarding local changes, and set repository defaults through the
   authorized API where available. Preserve old private `main`; never force,
   rewrite, delete or accidentally merge it into the public graph.


## Day-to-day changes

- Start public feature branches from `public-development` and merge public changes
  back into that clean branch. Keep the local checkout and repository default on
  `public-development` where access permits.
- Keep a clearly named public GitHub remote and canonical GitLab remote; push the
  same reviewed public branch/tag to both. Check the intended remote destination
  before pushing. A repository being private today does not make its source safe
  to merge publicly tomorrow.
- Do **not** merge the historical private `main` branch or another private-parent
  branch into public development. If an old public-safe change must be carried
  forward, review its content and recreate/cherry-pick that isolated change onto
  the clean public branch, then scan the resulting reachable history.
- Keep vendor drivers/PPDs, live configuration, credential hashes/keys, job state,
  private operator records and test captures outside this source tree. Examples
  use `example.test` and TEST-NET addresses. A public example must not accidentally
  name an active private service or widened real-client allow-list.
- There is no permanent history-sanitization/translation service. Subsequent public
  releases are ordinary descendants of the clean public branch.

## Release identity

A version tag `vVERSION`, binary, embedded `/source`, source asset, tar and Debian
package all refer to the **same exact public commit**. The pinned-input manifest
records its commit and tree. Complete docs/notices/scripts before the final build;
a documentation change after building requires a new final exact-source build.
Do not relabel an earlier binary with a newer source commit.

Before publication run the Rust quality/security suite, actual isolated/browser/
platform gates, locked-notice/source proof and disposable Linux package exercises.
Review the final reachable public history with a redacted secret scan and explicit
license/privacy inventory. Only then push the public branch/tag and publish assets.
Never change the private GitLab repository's visibility as a publication shortcut.

The version-tag workflow calls the quality workflow and is the **single** GitHub
release publisher after source/build/package gates. Do not race it with a manual
`gh release create` or a second tag-triggered quality/publisher job.

Before deployment, download the **actually published** assets and verify
SHA256SUMS, manifest/source identity and the binary extracted from the versioned
tar. Install that binary, then compare the running `/proc/PID/exe` checksum and
served `/source` against the published manifest. A successful local/private build
is not proof that a separately built CI binary is byte-identical.

