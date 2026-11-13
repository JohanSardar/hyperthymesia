# Guided setup: design notes

`hx-server setup` replaces the old by-hand first run. This file records the
reference we studied, what is automated, the state map and the visual direction.

## Reference: `agent-desktop permissions --guide` (1.2.0)

Tried on 2026-11-12 in a fresh Cirrus Labs macOS 26.6.2 VM (tart), launched from
Terminal, which had no permissions. Screenshots and a 60-second recording are kept
outside the repository with the setup evidence.

| Time | What happens |
| --- | --- |
| 0 s | The command returns at once. Its JSON reports the state *before* the guide (`"guide":"started"`, both permissions `denied`). Nothing later tells the caller what the user did. |
| 0–2 s | System Settings opens at Privacy & Security > Accessibility. A black panel docks at the top left, overlapping the Settings sidebar. |
| Panel | Terminal's icon with a hand cursor, the caption "Drag Terminal", `STEP 1 OF 2`, the title **Allow Accessibility**, the line **Drag Terminal into the list.**, the reason "agent-desktop needs this to see and use your apps." and a **Not Now** button. A red-to-purple-to-blue glow fills the bottom of the panel. |
| 2–4 s | The list loads. Terminal is *already listed*, switched off, yet the panel still says to drag it in. A ghost icon animates from the panel into the list, on a loop. |
| Switch | Turning Terminal on opens the system password prompt ("Privacy & Security is trying to modify your system settings. Enter your password to allow this."). The panel line changes to **Confirm with Touch ID or your password.** The ghost animation keeps running across the password prompt. |
| Cancel | The prompt closes, the switch stays off, the panel returns to "Drag Terminal into the list." with no word about the cancel. |
| Allowed | Within about 1.5 s the panel shows a green check, "Done.", for about a second, then moves on by itself. |
| Step 2 | Settings switches to Screen & System Audio Recording. Terminal is *not* listed this time, so dragging really is needed. The ghost icon animates toward the wrong list, "System Audio Recording Only". |
| Drop | Dropping the icon adds and enables Terminal (the password was remembered from step 1). macOS then shows its own "“Terminal” may not be able to record the contents of your screen until it is quit. Quit & Reopen / Later". Behind it the panel reads "You are all set. agent-desktop can now see and use your apps." with a leftover **Not Now** button. |
| End | The panel closes by itself about 2 s later. System Settings stays open. |
| Not Now | Ends the whole guide, not just the step. Settings stays open. Nothing reports the skip. |
| Run again | With both granted: prints `"guide":"not_needed"` and shows nothing. |

What to keep: the panel lives beside the real pane, one action per screen, a reason
line, automatic advance and close, skipping work that is done, the line changing
during the password prompt.

What to fix: report the final result to the caller; never ask for a drag when the
app can put itself in the list; say "switch it on" when that is all that is left;
keep animation off the password prompt; say what happened after a cancel; make
"Not now" skip one step; do not leave a stale button on the final screen; ask the
app itself, not a helper, so the request names the right app.

## What is automated and what only a person can do

Automated by `hx-server setup` (never shown as steps):

- Check the service is healthy.
- Pair the Mac app with this server: mint a one-use link and redeem it locally.
- Create the dashboard token and open the dashboard already signed in, through a
  one-use, five-minute sign-in link instead of a token to paste.
- Start the app and keep it starting at login.
- Put Hyperthymesia in each privacy list (its own request), so no drag is needed.
- Open the exact System Settings pane and the provider sign-in page.

Only a person (shown one at a time, easy first, only when not done yet):

1. Accessibility: switch Hyperthymesia on, confirm with Touch ID or password.
2. Input Monitoring: same, before macOS 26. From macOS 26 it is not a step: it
   comes with Accessibility, and TCC refuses to list it on its own while
   Accessibility is off (`authReason=4`), so it is never shown or counted. If
   Accessibility is refused or skipped, it is reported as not changed.
3. Screen Recording: macOS first shows its own "would like to record" alert (the
   guide says "Click Open System Settings."), then the switch as above. The guide
   then restarts the app itself, because a grant only takes effect in a new
   process, and waits for the first screenshot. macOS may ask once more ("…is
   requesting to bypass the system private window picker"); the guide says
   "Click Allow." If System Settings also asks "Quit & Reopen / Later", the
   guide says "Click Later.", because it already restarted the app.
4. Sign in to the model provider in the browser (Codex/ChatGPT or Grok).

Setup never clicks, approves or types into a system prompt. The guide panel never
takes keyboard focus, so every keystroke goes to the password prompt beside it.

The guide checks permissions through a fresh `hx-agent status` process (macOS
caches some answers per process) on a background queue, so the panel never stalls.

The guide runs in its own process group, so it keeps going while launchd restarts
the app; a second copy started by the restarted app exits on the guide lock.

## States

Each step has exactly one state, stored in the app's `setup/state.json`:

| State | Meaning |
| --- | --- |
| `pending` | Not reached yet. |
| `already_done` | True before setup touched it (checked for real). |
| `waiting_user` | Its pane or page is open, waiting for the person. |
| `waiting_system` | The password prompt is up, or the sign-in is being finished. |
| `done` | Became true during this run, checked for real. |
| `skipped` | The person chose Not now. Everything else still works without it. |
| `refused` | The person cancelled the password prompt or closed the sign-in. Not retried until the next run. |
| `failed` | Something broke; the step carries the error. |

The run itself is `running`, `finished` or `interrupted`. Quitting halfway (closing
the guide, quitting the app, macOS's Quit & Reopen, a reboot) leaves the run
`interrupted`; the next app start resumes it, and the next `hx-server setup` starts
a new run that skips everything already true.

## Visual direction

Based on macOS's own privacy prompts and System Settings itself: an opaque panel in
the system window colour, San Francisco type, the same SF Symbol the Settings
sidebar uses for the pane, black or white text at full contrast, the system accent
only on the focus ring. It should read as part of the screen it sits beside, not a
product. No glow, gradient, badge, card stack or status dot. The progress line is
plain text ("Step 1 of 3"). Done is a single green system checkmark.
