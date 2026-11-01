# Changelog

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
