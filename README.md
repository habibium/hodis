# hodis

Web search from the terminal, for people and for AI agents.

hodis searches the web and prints what it finds: styled for reading in a
terminal, or as JSON, Markdown, plain text or bare URLs for scripts and notes.
It searches through SearXNG, either an instance you run or one hodis sets up for
itself, or gets Google results from Serper with an API key. `hodis mcp` offers
the same search to AI agents as an MCP tool.

```console
$ hodis capital of norway
1. Oslo - Wikipedia [google]
https://en.wikipedia.org/wiki/Oslo
Oslo is the capital and largest city of Norway. It constitutes both a county and a municipality.

2. Norway - Wikipedia [google]
https://en.wikipedia.org/wiki/Norway
Its capital and largest city is Oslo. The country shares a long eastern border with Sweden.
```

## Install

hodis has no releases yet. Install it from source with Rust 1.96 or newer:

```sh
cargo install --git https://github.com/habibium/hodis
```

If no SearXNG instance answers at `localhost:8888`, the first search sets one up,
which needs `tar`. macOS, Windows 10 and later, and most Linux systems ship it.

## Searching

```sh
hodis rust borrow checker                # every word is part of the query
hodis --page 2 rust borrow checker
hodis --format urls rust borrow checker
hodis -- key                             # search for a word that is a hodis command
```

| `--format` | Output                                              |
| ---------- | --------------------------------------------------- |
| `pretty`   | Styled text for reading in a terminal (the default) |
| `json`     | A versioned JSON document for scripts and agents    |
| `markdown` | Markdown for notes and documents                    |
| `plain`    | Text without colors, numbering or engine names      |
| `urls`     | Result URLs, one per line                           |

`--color auto|always|never` sets when output is styled. With `auto`, the
default, it is styled only in a terminal, and never when `NO_COLOR` is set.
Warnings, such as a list of engines that did not answer, go to stderr and stay
out of the results.

## Providers

### SearXNG

[SearXNG](https://docs.searxng.org) is a free metasearch engine: it queries
other search engines and merges their results. hodis looks for an instance at
`http://localhost:8888/`. If nothing answers there, hodis installs one of its
own: the first time, it downloads a Python interpreter and SearXNG, a few
hundred megabytes, into its data directory. Later searches reuse that instance,
which listens on 127.0.0.1 only and keeps running between searches.

To use another instance, name it in the config file. Setting `url` turns off
hodis's own instance unless `embedded = true` is set as well.

```toml
[searxng]
url = "https://searxng.example.org/"
```

The instance must serve JSON, which public instances often refuse: `json` has
to be listed under `search.formats` in its `settings.yml`.

### Serper

[Serper](https://serper.dev) serves Google results through an API, with a key
from your account there. hodis keeps the key in the system keychain:

```sh
hodis key set serper                     # prompts for the key, or reads stdin
hodis --provider serper rust borrow checker
```

hodis never writes the key to a file. Where there is no keychain, set
`SERPER_API_KEY` instead; when it is set, it takes precedence over a stored key.
`hodis key delete serper` removes the stored key, and `provider = "serper"` in
the config file makes Serper the default.

## Configuration

The config file is `~/.config/hodis/config.toml` on Linux and macOS and
`%APPDATA%\hodis\config.toml` on Windows; `hodis --help` names it, and
`HODIS_CONFIG` points hodis at another. Every setting is optional, and flags
override the file. These are the defaults:

```toml
provider = "searxng"   # or "serper"
format = "pretty"      # pretty, json, markdown, plain or urls
color = "auto"         # auto, always or never
timeout = 10           # seconds allowed for each search request

[searxng]
url = "http://localhost:8888/"
embedded = true        # set up hodis's own instance when nothing answers at url

[serper]
url = "https://google.serper.dev/"   # only a proxy in front of Serper needs another
```

hodis warns about keys it does not know, such as a misspelled one.

## JSON output

`--format json` prints a document for scripts. Its shape changes only with its
`schema_version`: the major part goes up when a field is removed, renamed or
retyped, and the minor part when one is added.

```json
{
  "schema_version": "1.0.0",
  "query": "capital of norway",
  "answer": null,
  "results": [
    {
      "title": "Oslo - Wikipedia",
      "url": "https://en.wikipedia.org/wiki/Oslo",
      "snippet": "Oslo is the capital and largest city of Norway.",
      "image": null,
      "source": "google"
    }
  ],
  "provider": "searxng",
  "diagnostics": {
    "unresponsive_engines": [{ "engine": "brave", "reason": "too many requests" }],
    "skipped_results": 0
  }
}
```

`answer` holds a `url` and `content` when the search has a direct answer, such
as a fact or a definition. `diagnostics` lists the engines that did not answer
and counts the results dropped for lacking a title or a valid URL.

## AI agents

`hodis mcp` is an [MCP](https://modelcontextprotocol.io) server on standard
input and output. Its one tool, `web_search`, takes a `query` and an optional
`page`, and returns the JSON document above.

To add it to Claude Code for every project:

```sh
claude mcp add --scope user hodis -- hodis mcp
```

Clients configured with JSON, such as Claude Desktop and Cursor, take an entry
like this:

```json
{
  "mcpServers": {
    "hodis": { "command": "hodis", "args": ["mcp"] }
  }
}
```

`hodis mcp --provider serper` gives agents Google results while your own
searches stay on SearXNG. If hodis is going to set up its own SearXNG, run one
search in a terminal first: the setup can take longer than a client waits for a
tool.

## Files

hodis keeps its SearXNG instance in its data directory: `~/.local/share/hodis`
on Linux and macOS, `%APPDATA%\hodis` on Windows, or wherever `HODIS_DATA_DIR`
points. To stop the instance, end the process whose ID is in
`engine/searxng.pid` there; on Linux or macOS:

```sh
kill "$(cat ~/.local/share/hodis/engine/searxng.pid)"
```

Once it has stopped, deleting the data directory removes it altogether.

## License

[MIT](LICENSE)
