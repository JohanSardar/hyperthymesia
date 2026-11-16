# Open-source preparation decisions

## 2026-11-08

1. Compare with Vercel Labs fx, not a JS framework: native CLI, agent integration,
   binaries and curl installation are the relevant user journey. Do not copy its
   branding, permissive license or undocumented claims.
2. Keep AGPL-3.0-only and `publish = false`. The requested release is a GitHub
   binary release, never a registry publication.
3. Preserve runtime behavior and existing SQLite migration/rollback semantics.
   An open-source cleanup is not permission to redesign data storage or capture.
4. Preserve pre-existing untracked `.claude/` and ignored build directories.
   Remove tracked internal plans/logs only through reviewed PRs. Never clean
   unknown local files or rewrite published history.
5. CI already exists, so harden it in the first PR rather than introduce a
   competing pipeline. Require one stable aggregate check with strict branch
   freshness and admin enforcement; do not require a second human reviewer
   on this single-maintainer repository.
6. Use patch version 0.1.3 for the first binary release: the actual server/app
   packages are already 0.1.2, despite the library workspace default of 0.1.0.
   Correct the initial 0.1.1 assumption rather than downgrade. No incompatible
   public behavior is intended. Keep the app and server versions equal.
7. Keep full privacy/cost/host-execution warnings, even though they move below
   the concise README introduction. Quick start proves local storage/service,
   not model quality, capture permissions or a paid provider request.
8. Host the installer via branch-based GitHub Pages from main `/docs` with
   `.nojekyll`. Pin the release checksum manifest's SHA-256 in a thin launcher,
   then verify and execute the same release's existing lifecycle installer.
   This binds all platform assets and installer code with one reviewed pin.
9. Never install a service on the developer's real HOME to demonstrate safety.
   Use fixtures for failure injection and disposable hosted runners for real
   service/live-download tests. Missing Docker locally is not a passing test.
10. Record each later implementation choice here, along with limits. Do not
    manufacture clean-machine, real-provider or macOS-permission evidence.
11. Enforcing the omitted shell-hook tests exposed missing zsh on Ubuntu.
    Install it on the disposable runner, not the developer host; pin Bun to
    the locally validated 1.3.12 instead of silently following latest.
12. Resume the existing prepared branches in place, preserving Factory Droid's
    commits and authorship. Reconcile the already-squashed installer with main;
    do not rebuild completed areas or merge stale branches without checks.
13. Publish the GitHub binary release only from the approved annotated v0.1.3
    tag at 545d2224c325c02da9de4400c30d77cacd4b9e30. Verify all six downloaded
    public assets independently before changing the separate Pages pin.
14. Enable branch-based Pages from main /docs through the GitHub API. Keep the
    public launcher fail-closed until the VERSION/SHA256-only PR is green.
15. Keep remaining large-module debt and untested real-provider/Mac permission
    flows explicit; synthetic or login-free checks do not establish them.
16. 2026-11-12: the GitHub account was suspended, so every GitHub URL returns
    404. The source moved to https://gitlab.com/johansardar/hyperthymesia (public)
    and everything that downloads or links moved to `hyperthymesia.eilmer.si`,
    a GitLab Pages custom domain. Release assets live in the project's generic
    package registry; the Pages job copies them into the site after checking them
    against the pinned manifest. Records of earlier GitHub runs stay as history.
