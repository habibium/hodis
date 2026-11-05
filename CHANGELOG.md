# Changelog

## 0.2.0 - 2026-11-05

- The first run in a terminal is now a setup wizard: it asks which provider to
  use, then only what that provider needs, tests it with one search, and saves
  the choice. `hodis setup` runs it again.
- Setting up SearXNG shows a progress bar over a running list of what is being
  added and changed, in the style of apt and pacman, and keeps the full output
  in `engine/setup.log`.
- A SearXNG setup cut short can now be retried; before, it failed from then on
  until the data directory was deleted by hand.
- Setting up SearXNG no longer writes outside the data directory: uv used to add
  a `python3.13` launcher to `~/.local/bin` and, on Windows, a registry entry.
- Setting up SearXNG no longer needs `tar` on the machine.
- SearXNG gets three minutes to start rather than one, is compiled ahead of its
  first start, and reports the end of its log if it stops or does not answer.
- `setup` is now a command, so searching for that word alone takes `hodis -- setup`.
- `embedded = "only"` under `[searxng]` always searches hodis's own instance, even
  when something else answers at `url`. `hodis setup` saves it when you choose
  that instance, so another SearXNG on `localhost:8888` cannot take its place.
- Dependencies are updated past 13 security advisories, among them rustls and h2,
  which carry all of hodis's HTTPS.
- The library's API changed: `SearXNG::embedded` is now the `Embedded` enum,
  `Engine::ensure_running` reports to a `Progress`, and `render_setup_step` is gone.

## 0.1.0 - 2026-11-01

The first release, for testers.

- `hodis <query>` searches the web and prints the results styled for a
  terminal, or as JSON, Markdown, plain text or bare URLs with `--format`. The
  JSON document is versioned, starting at schema 2.0.0.
- `hodis` without a query opens a full-screen search: type a query, move
  through the results, open one in the browser or copy its link.
- With no provider chosen, hodis searches SearXNG when an instance answers at
  `localhost:8888`, and DuckDuckGo otherwise, so it needs nothing installed.
  `--provider` picks SearXNG, DuckDuckGo or Serper, which serves Google results
  with an API key kept in the system keychain, or several of them at once.
- Choosing SearXNG with none running sets up hodis's own instance.
- `hodis mcp` serves the same search to AI agents over MCP.
- Results are reused for an hour; `--no-cache` searches afresh.
- A config file sets the defaults for all of the above.
