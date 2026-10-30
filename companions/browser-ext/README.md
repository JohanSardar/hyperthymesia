# Browser companion

From this directory:

```sh
~/.bun/bin/bun install --frozen-lockfile
~/.bun/bin/bun run build
~/.bun/bin/bun run typecheck
~/.bun/bin/bun test
```

Open `chrome://extensions`, enable Developer mode, select **Load unpacked**, and
choose this directory's `dist` folder. Reload existing tabs after installing.
The agent must separately register the native host `com.hyperthymesia.agent`
with this extension's ID in its origin allowlist. The extension sends one
CompanionEvent per native message and removes it from the durable queue only
when the host responds with `{ "ok": true }`, after durable acceptance. Other
replies/errors/timeouts retain it. Confirm that ack shape with the native agent
before deployment; the envelope schema does not specify acknowledgments.

Events include active/background visits, closed interval revisions, viewport
text and SHA-256 digests. A visit is visible only with an active tab, focused
window, visible document and active idle state. On navigation, visibility starts
unknown/false until the top-frame content script reports it. A viewport is
partial even without truncation: it is never a claim of complete page coverage.
Text is limited to 16,000 characters and 5,000 visited text nodes. Input,
textarea, editable, hidden and offscreen elements are omitted. No keystrokes or
screenshots are read. Text-node boundaries can include text beyond a clipped
line; snapshots are bounded viewport samples, not exact pixel reconstructions.

Use the extension's **Options** to add URL globs, one per line. The built-in
banking/payment exclusions in `src/core.ts` always apply (they are an explicit
list, not a classifier for every financial site). Non-HTTP(S), credential-bearing
URLs and incognito pages are suppressed. Policy is checked before text extraction
and again before persistence. Changing exclusions purges pending matching events
and saved visit state; previously delivered records require the agent's policy.

`chrome.storage.local` holds pending events and visit state; closed revisions
have the same source key as their opening event and a different revision. Visits
and pending records commit in one storage update. Retries occur every 30 seconds
and on browser events. Delivery is at least once: a lost ack can resend the same
source key/revision, so the agent must deduplicate. Storage is subject to Chrome's
quota; storage errors stop writes without evicting unacknowledged records. A
terminated worker cannot observe events until Chrome wakes it; intervals across
that gap are exposure observations, not proof of engagement or complete coverage.

Tests use a mocked Chrome API for worker lifecycle/persistence and installed
Google Chrome (`channel: 'chrome'`) for the bundled content script's DOM behavior,
with mocked extension messaging. No browser binary download. Chrome DOM tests
are explicitly skipped if Chrome is absent. Build before tests. Full unpacked
MV3/native-host integration still requires a configured native host and browser
profile; installed Chrome did not expose a worker through command-line extension
loading in the development smoke attempt.
