# Setting up Hyperthymesia for someone: instructions for AI agents

Use this when a person asks you to install or set up Hyperthymesia on their
computer. You do the work. They only do what macOS or their model provider
requires a person to do.

## Run it

1. Install without the interactive hand-off, so you can read the progress:

   ```sh
   curl -fsSL https://hyperthymesia.eilmer.si/install | sh -s -- --no-setup
   ```

2. Run guided setup and read its output line by line:

   ```sh
   ~/.local/bin/hx-server setup --json
   ```

   Each line is one JSON object. `{"event":"step", ...}` lines carry `step`,
   `state` and `say`. The last line is `{"event":"finished","report":{...}}`.
   Setup waits up to 20 minutes for the person; the exit code is `0` when
   everything is done, `3` when something was skipped, refused or left unfinished.

Setup pairs the Mac app, signs the dashboard in, opens the exact System Settings
pane or sign-in page, puts Hyperthymesia in each list, waits, and checks every
result for real. A small panel beside the pane tells the person what to do.

## Rules

- Do every step yourself. Never hand the person commands, links, file paths or
  instructions to follow.
- While setup waits, tell the person only the latest `say` text, for example
  "Switch on Hyperthymesia in Accessibility." or "Confirm with Touch ID or your
  password." Say nothing else while waiting.
- Never click Allow, switch a permission on, approve a prompt, type a password or
  sign in for the person, even if you have tools that could.
- If a step comes back `skipped` or `refused`, accept it. Do not retry, argue or
  explain how to do it by hand. Everything else keeps working without it.
- When `finished` arrives, report in one or two sentences: what was allowed, what
  was already set, what was skipped or not changed, and whether `report.proof`
  shows the server receiving data. Offer to run `hx-server setup --json` again
  later for anything left.

## Running it again

`hx-server setup` is safe to repeat. It skips everything already true and only
shows what is left. If the person quit halfway, the Mac app resumes the guide by
itself; a new `hx-server setup` run also picks up where they stopped.
