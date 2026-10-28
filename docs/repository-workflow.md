# Public source and repository workflow

Ferriprint's public development branch is `public-development` at
<https://github.com/ressl/ferriprint>. The canonical GitLab repository remains
private. Both destinations are intended to receive the **same reviewed public
lineage**, never a merge of the old private pilot history. Record the exact
observed ref separately for each provider; a successful GitHub push does not
establish a new GitLab sync.

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
- Keep a clearly named public GitHub remote and canonical GitLab remote. Check
  the intended destination and push only the approved public branch/tag. Record
  each provider's actual resulting ref, rather than inferring mirror success.
  If an approved mirror is unavailable, retain its pending commit and respect
  the transport/authentication boundary; do not bypass policy to claim sync.
  A repository being private today does not make its source safe to merge
  publicly tomorrow.
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
records its commit and tree. **Before publication**, complete docs/notices/scripts
before the final build; any tracked documentation change after that build requires
a new final exact-source build before publishing those artifacts.
For multiarchitecture releases, both native ELF binaries/tars/packages have
architecture-specific manifests but the same public commit/tree and byte-identical
source/SPDX. Different common bytes block publication; never silently choose one.
The QA-only loopback receiver is not a release asset. Merge only reviewed expected
assets into one final checksum set after both native package/runtime gates.
Do not relabel an earlier binary with a newer source commit.

**After publication**, a documentation-only descendant is a separate source
commit. It may record later observations, but must not retag the immutable
release, replace its assets or relabel their embedded source as that descendant.
Such documentation does not require a replacement release build or tag.

Before publication run the Rust quality/security suite, actual isolated/browser/
platform gates, locked-notice/source proof and disposable Linux package exercises.
Review the final reachable public history with a redacted secret scan and explicit
license/privacy inventory. Only then push the public branch/tag and publish assets.
Never change the private GitLab repository's visibility as a publication shortcut.

The version-tag workflow calls the quality workflow and is the **single** GitHub
release publisher after source/build/package gates. Do not race it with a manual
`gh release create` or a second tag-triggered quality/publisher job.

The private GitLab destination is intended to receive both the same reviewed
source branch/tag and the actual released packages. Push only the approved
`public-development` ref and the same version-tag object where authorized access
is available; record each provider's observed outcome. Never push all refs,
import private parents or change project visibility. A blocked mirror remains
pending, independently of an already successful publication at the other provider.

After the sole GitHub publisher succeeds, download **all nine actual assets**
into an empty private directory. Verify the single eight-entry `SHA256SUMS`,
both manifests/ELFs and embedded source against the approved commit. Mirror those
exact downloaded files, including `SHA256SUMS`, into the existing private GitLab
generic registry under `ferriprint/VERSION/FILENAME`, then create the GitLab
release on the already-pushed same tag with links to those private objects.
Use existing protected API authentication, not token argv/logs or a new publisher.
Download the private copies and require byte-identical files/checksums before
calling the mirror complete. Do not rebuild, relabel, overwrite a mismatching
existing asset or replace the GitHub workflow with manual release creation.

Before deployment, download the **actually published** assets and verify
SHA256SUMS, manifest/source identity and the binary extracted from the versioned
tar. Install that binary, then compare the running `/proc/PID/exe` checksum and
served `/source` against the published manifest. A successful local/private build
is not proof that a separately built CI binary is byte-identical.

