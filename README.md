# cc-sessions

One place to see, close and resume [Claude Code](https://claude.com/claude-code) sessions
across every account on a machine, from a local web page or the command line. It also
adds, renames, re-logs-in, removes and restores the accounts themselves.

An account is a Claude Code config folder (`~/.claude`, `~/.claude-work`, ...) with a name
and a shortcut command. `claude agents` only sees one account at a time; cc-sessions asks
each account in turn and shows the result together, grouped by project folder.

It is a single small program with nothing else to install, for macOS, Linux and Windows.
It is an unofficial tool and is not affiliated with Anthropic.

## See it

[![The Sessions view: a folder tree, sessions from three accounts in one list, and the details of the picked session beside it](docs/demo.png)](docs/demo.mp4)

The screenshot shows the classic layout. The one-minute film
([docs/demo.mp4](docs/demo.mp4)) introduces 2.5: the Finder layout, Usage, Settings and
accounts. Both use made-up demo data.

## Install

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/barath-dev/cc-sessions/main/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/barath-dev/cc-sessions/main/install.ps1 | iex
```

The installer downloads the program for your system, checks it against the release's
SHA-256 list, and asks a few setup questions. Each has a default, so pressing Enter through
them works:

1. **Command name** — what you type to start it (default `cc-sessions`).
2. **Port** for the page (default `4777`).
3. **Clickable shortcut** in Applications or the Start menu (default yes).
4. **Register the Claude Code folders it finds** as accounts (default yes).
5. **Manage the `claude-<name>` shortcuts** that start each account (default yes). It shows
   the one line it would add to your shell's startup file, and keeps a backup.

Run the install line again to update; settings are kept. Run `cc-sessions setup` to change
any answer later.

## Start and stop

```
cc-sessions          # start the server if needed and open the page
cc-sessions stop     # stop the server (same as Disconnect in the page)
```

The server runs in the background on `http://127.0.0.1:4777`, so the terminal can be
closed. Nothing starts at login. It stops by itself after 30 minutes with no page open,
and **Disconnect** (in the classic header, or Settings › General) stops it straight away.
Your Claude sessions keep running either way.

## The page

The page has two layouts, picked in **Settings › Appearance › Layout**; it opens in the one
you used last. Both show the same sessions and accounts; alerts are in the classic layout
and token usage in the Finder layout.

### Classic layout

- **One list** of every session in every account: pinned ones first, then the ones open
  now, then saved conversations by when they were last active (or A–Z).
- **A folder tree** beside it. Pick a folder to see only what is under it; pin one to keep
  it at the top. **Needs you** lists sessions that are blocked, or finished in the last day
  and not yet closed, and the browser tab shows how many.
- **Search** covers names, folders, artifacts and what you typed in each conversation.
  Account and status chips narrow the list.
- **Details** open in a panel beside the list when you click a session: its prompts,
  artifacts (links to claude.ai, which open for the Claude login that published them) and
  the command that resumes it.
- **Actions**, on each row as you point at it: rename, copy the resume command, pin, and
  **Close** (stops the session, keeps its conversation) or **Resume** (reopens it in a new
  terminal window). Tick open sessions to close several at once.
- Conversations that were opened and never used are hidden, with a link to show them.
- **Alerts**, in the header, show a desktop notification when a session starts waiting for
  you or finishes. They need the page open in a browser tab.

### Finder layout

A Finder-style browser of the same sessions: folders you open like folders, sessions as
files in them, accounts as tags in the sidebar.

- **Icons, List and Columns** views, back and forward, a path bar, and Recents, Needs you
  and Open now in the sidebar.
- **Quick Look** (Space or double-click) shows a session's details and actions; a
  right-click menu has the rest.
- Keys as in Finder: arrows to move, Enter to rename, ⌘↑ for the folder above, ⌘[ and ⌘]
  for back and forward, ⌘1–3 for the views, ⌘O to resume, ⌘⌫ to close (after asking),
  ⌘, for Settings.
- **Usage** shows the tokens your sessions used: today, the last 7 and 30 days, a calendar
  heat map of every day, when in the week you use Claude, and totals by account, model
  and session. Pick all tokens, without cache reads, or output only. The heat maps come in
  blue, green, amber, purple, teal or your primary colour. **3D** raises the calendar into
  a lit 3D object, a block per day as tall as its tokens, with height lines and the busiest
  day and today marked. Pick a view (Angled, Front, Top, Side, Last 30 days, or keys 1–5),
  drag to turn it, Shift-drag to move it, ⌘-scroll to zoom, or show it full screen. Click a
  day (or move to it with the arrow keys and press Enter) to see its tokens by kind, hour
  and account, how it compares with its week and the year, and the sessions that used them.

### Both layouts

- **Rename** a session with the pencil on its row, by double-clicking its name, or with
  Enter in the Finder layout. The name is kept by cc-sessions and shown in the page and on
  the command line; Claude Code keeps its own title. Clear the name to go back to that title.
- **Settings**, from the classic header or the Finder sidebar:
  - **Appearance**: the layout; Light, Dark or Match system; a template (Mac, Classic,
    Graphite or Paper, or each layout's own look); and a primary colour for buttons, links
    and what is selected. These are kept in the browser.
  - **Accounts**: login, plan, shortcut and folder of each account. Add, rename, change
    login, remove and restore. **Edit…** opens one dialog for an account's name, alias,
    colour (which marks it everywhere), login and removal.
  - **General**: disconnecting, which stops the server.
- **From the keyboard** in the classic layout, **Skip to sessions** is the first stop, the
  arrow keys move through the list and the folder tree, and Esc closes the details.

Statuses: `Working` (Claude is busy), `Blocked` (waiting for your input), `Idle` (open in
a terminal, doing nothing), `Done` (background task finished, not closed), `Closed`
(stopped through this tool, or stopped in Claude Code).

## What it never does

- It never deletes or moves anything under an account folder. The test suite checks this
  on every run.
- **Remove account** only hides the account and drops its shortcut. The folder, login and
  history stay on disk, and **Restore** brings it back.
- **Close** only stops a session. Its transcript stays and it can be resumed.
- Renaming an account or its shortcut changes nothing inside the account folder.
- Renaming or pinning a session changes nothing in its transcript. Names and pins are kept
  in `titles.json` and `pins.json`.
- Its own records are rewritten in place with the previous version kept as `<file>.bak`.

The only files it removes are its own launchers: its pid file, a command link it created
when you change the command name, and on Windows the `claude-<name>.cmd` launcher of an
account you renamed or removed. None of them hold data.

Claude Code itself deletes conversations after `cleanupPeriodDays` (30 by default) without
activity. Settings › Accounts shows that number per account. To keep history longer, set
`cleanupPeriodDays` in that account's `settings.json`.

## Command line

```
cc-sessions list [--folder PATH] [--account NAME] [--open | --closed | --all] [--json]
cc-sessions history [--folder PATH] [--account NAME] [--by-date] [--json]
cc-sessions close <id>... | --folder PATH | --account NAME  [--yes]
cc-sessions rename <id> NEW NAME | --reset
cc-sessions account list [--removed]
cc-sessions account add NAME [--alias A] [--dir PATH] [--no-login]
cc-sessions account rename NAME NEW-NAME [--alias A]
cc-sessions account login NAME
cc-sessions account remove NAME [--yes]
cc-sessions account restore NAME [--alias A]
cc-sessions setup [--yes] [--command NAME] [--port N] [--shortcut | --no-shortcut] [--no-import] [--no-aliases]
```

Session ids are shown as `account:id`. `close` asks before closing, asks separately for
each interactive session, and skips the session it is run from. `--yes` only skips the
question for background sessions. `history` groups by folder; `--by-date` prints one list,
newest first. `rename` takes the id `history` shows, and `--reset` gives a session its own
title back.

## Differences between systems

| | macOS / Linux | Windows |
|---|---|---|
| Settings and records | `~/.config/cc-sessions` | `%APPDATA%\cc-sessions` |
| Program | `~/.local/share/cc-sessions/cc-sessions` | `%LOCALAPPDATA%\cc-sessions\cc-sessions.exe` |
| Account shortcuts | aliases loaded by one line in `~/.zshrc` or `~/.bashrc` (fish: a `conf.d` file) | `claude-<name>.cmd` launchers on your PATH |
| Closing an interactive session | asks it to exit, and leaves it if it has not within 5 seconds | ends it immediately; Windows offers no gentler way |
| Resume opens | iTerm or Terminal; on Linux the first terminal program found | Windows Terminal, else Command Prompt |

On a Linux machine with no desktop (for example over SSH), Resume cannot open a terminal
window; the Copy button still gives the command. Claude Code running inside WSL is covered
by installing the Linux version inside WSL.

The Windows program is not code-signed, so antivirus software may occasionally flag it.

## Files it keeps

In the settings folder above:

| File | Purpose |
|---|---|
| `config.json` | Answers from setup: command name, port, shortcut, idle limit. |
| `accounts.json` | Account registry: name, folder, shortcut, colour, removed flag. |
| `aliases.sh` | Generated account shortcuts (macOS, Linux). |
| `icon.png`, `icon.ico` | Icon of the clickable shortcut (Linux, Windows). |
| `closed.json` | Sessions closed through the tool. |
| `titles.json` | Names you gave to sessions. |
| `pins.json` | Sessions and folders you pinned. |
| `history-cache.json` | Rebuildable index of saved conversations, with their token counts. |
| `ui-token` | Access key for the web app (readable only by you). |
| `ui.pid`, `ui.log` | The running server's pid and its log. |

`idleMinutes` in `config.json` sets how long the server waits with no page open before it
stops; `0` keeps it running until you stop it.

The layout, theme, template and primary colour are kept in the browser, not here, so
another browser starts with the defaults.

## How the server is protected

The page can end processes, so the server listens on `127.0.0.1` only, requires the token
from `ui-token` on every API call, rejects requests whose `Host` is not
`127.0.0.1:<port>` or `localhost:<port>`, and only accepts JSON `POST`s for changes.

## How sessions are read and closed

- Sessions come from `claude agents --json --all`, run once per account. The default
  account runs with `CLAUDE_CONFIG_DIR` unset; the others with it set to their folder.
- Background sessions are closed with `claude stop <id>`. For an interactive one, the tool
  first checks that the process id still belongs to that session, then ends it.
- Claude Code has no "closed" state for interactive sessions (they vanish once their
  process ends) and leaves a finished background session as `done` after `claude stop`.
  `closed.json` is what keeps both listed as closed.
- History is read from `<account>/projects/<folder>/<session>.jsonl`, top level only,
  read-only. Artifacts are the links Claude Code records there when a conversation
  publishes or opens one.
- Token counts come from the usage Claude Code records on each reply in those files. A
  reply written over several lines is counted once. Sub-agents keep conversations of their
  own, which are not counted, and days older than an account's `cleanupPeriodDays` are
  empty because Claude Code has deleted them.

## Building from source

```sh
cargo build --release      # target/release/cc-sessions
cargo test                 # unit tests, and end-to-end tests against a fake claude
```

Everything that differs between systems is in `src/os/`. The page is in `ui/` and is built
into the program; set `CC_SESSIONS_UI_DIR=ui` to serve it from disk while working on it.
`CC_SESSIONS_HOME` and `CC_SESSIONS_PORT` point a run at a different settings folder and
port.

## Licence

MIT. See [LICENSE](LICENSE). The icons are from [Lucide](https://lucide.dev) (ISC licence;
the notice is in `ui/icons.js`).
