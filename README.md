# hodis

Web search from the terminal, for people and for AI agents.

![hodis printing results for a search, piping their URLs to head, then browsing the same results in its full-screen view](docs/demo.gif)

```console
$ hodis capital of norway
1. Oslo - Wikipedia [google]
https://en.wikipedia.org/wiki/Oslo
Oslo is the capital and largest city of Norway. It constitutes both a county and a municipality.

2. Norway - Wikipedia [google]
https://en.wikipedia.org/wiki/Norway
Its capital and largest city is Oslo. The country shares a long eastern border with Sweden.
```

- Prints results styled for reading, or as JSON, Markdown, plain text or bare
  URLs for scripts and notes.
- Opens a full-screen search when run without a query, for moving through
  results and opening them in your browser.
- Searches DuckDuckGo with nothing to set up, SearXNG (an instance you run, or
  one hodis sets up for itself), or Google through Serper with an API key.
- Offers the same search to AI agents as an MCP tool.

## Contents

- [Install](#install)
- [First run](#first-run)
- [Searching](#searching)
- [Interactive search](#interactive-search)
- [Providers](#providers)
  - [DuckDuckGo](#duckduckgo)
  - [SearXNG](#searxng)
  - [Serper](#serper)
  - [Several at once](#several-at-once)
- [AI agents](#ai-agents)
- [Configuration](#configuration)
- [JSON output](#json-output)
- [Files](#files)
- [Troubleshooting](#troubleshooting)
- [License](#license)

## Install

hodis needs nothing else installed, even to set up its own SearXNG.

### Install script

On Linux or macOS:

```sh
curl -fsSL https://github.com/habibium/hodis/releases/latest/download/hodis-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://github.com/habibium/hodis/releases/latest/download/hodis-installer.ps1 | iex
```

The scripts check the download against its checksum, install hodis into
`~/.local/bin` (on Windows, `%LOCALAPPDATA%\Programs\hodis`) and put it on your
PATH. Two environment variables change that:

| Variable            | Effect                                  |
| ------------------- | --------------------------------------- |
| `HODIS_INSTALL_DIR` | Installs into another directory         |
| `HODIS_VERSION`     | Installs another release, like `v0.1.0` |

### Cargo

With Rust 1.96 or newer:

```sh
cargo install hodis
```

### Archives

The [releases page](https://github.com/habibium/hodis/releases) has static
builds for Linux on x86_64 and ARM, macOS on Intel and Apple silicon, and
Windows on x64.

## First run

The first time you run `hodis` in a terminal, it asks which provider to use,
then only what that provider needs:

| Provider   | What it asks                                          |
| ---------- | ----------------------------------------------------- |
| DuckDuckGo | Nothing                                               |
| SearXNG    | Whether to use an instance you run, or to install one |
| Serper     | Your API key, hidden as you type                      |

It then tests the provider with one search and shows each thing it adds or
changes as it goes:

```text
(4/8) Downloading SearXNG 2026.8.14
      unpacked 988 files into engine/source
(5/8) Installing Python packages
      + babel 2.18.0
[ 57%] [###########---------] 0:03 / Installing Python packages
```

Your choice goes into the [config file](#configuration). A SearXNG address or a
Serper key that fails the test is not saved. DuckDuckGo and hodis's own SearXNG
are saved with a warning, as a failed search there usually means a search engine
turned it away for now.

Run `hodis setup` to change provider or to retry a setup that failed. Pressing
esc at the first question leaves everything as it was, and hodis asks again next
time.

hodis never asks when:

- a search's output goes to a file or a pipe
- `--provider` names a provider
- a config file already exists

So scripts and agents are never held up.

## Searching

```sh
hodis rust borrow checker                # every word is part of the query
hodis --page 2 rust borrow checker
hodis --format urls rust borrow checker
hodis --no-cache rust borrow checker     # search afresh, past the cache
hodis -- key                             # search for a word that is a hodis command
```

| `--format` | Output                                              |
| ---------- | --------------------------------------------------- |
| `pretty`   | Styled text for reading in a terminal (the default) |
| `json`     | A versioned [JSON document](#json-output)           |
| `markdown` | Markdown for notes and documents                    |
| `plain`    | Text without colors, numbering or engine names      |
| `urls`     | Result URLs, one per line                           |

`--color auto|always|never` sets when output is styled. With `auto`, the
default, it is styled only in a terminal, and never when `NO_COLOR` is set.

Warnings, such as a list of engines that did not answer, go to stderr and stay
out of the results.

hodis keeps each search's results for an hour, so repeating a search is instant
and costs no Serper credit. `--no-cache` searches afresh and keeps the new
results.

## Interactive search

Run `hodis` in a terminal without a query to search full-screen: type a search,
press enter, and move through the results. `hodis -i rust borrow checker` opens
the view with that search made.

| Key                | Action                                          |
| ------------------ | ----------------------------------------------- |
| `↑` `↓` or `j` `k` | Select a result; `PgUp` `PgDn` and `g` `G` jump |
| `enter` or `o`     | Open the selected result in your browser        |
| `y`                | Copy its link                                   |
| `n` `p`            | Show the next or previous page of results       |
| `/`                | Edit the search; `esc` goes back to the results |
| `r`                | Search again, past the cache                    |
| `q` or `esc`       | Quit                                            |

The search field takes the usual line-editing keys, such as `ctrl-w` to delete a
word and `ctrl-u` to clear the line. `--provider`, `--page`, `--no-cache` and
`--color` work as they do for printed results.

Copying goes through the terminal, with the OSC 52 escape sequence, so it
reaches your clipboard over SSH too.

## Providers

|                | DuckDuckGo | SearXNG                              | Serper                 |
| -------------- | ---------- | ------------------------------------ | ---------------------- |
| Results from   | DuckDuckGo | Many search engines, merged          | Google                 |
| Setup          | None       | Your instance, or one hodis installs | An API key             |
| Cost           | Free       | Free                                 | Credit on your account |
| Direct answers | No         | Yes                                  | Yes                    |

To choose one, run `hodis setup`, pass `--provider`, or set `provider` in the
config file. With none chosen, hodis searches SearXNG when an instance answers
at `http://localhost:8888/`, or at the address the config file gives, and
DuckDuckGo otherwise.

### DuckDuckGo

```sh
hodis --provider duckduckgo rust borrow checker
```

DuckDuckGo offers no search API, so hodis reads the results page it serves to
browsers without JavaScript, as SearXNG does. Heavy use can get you a
[CAPTCHA](#troubleshooting) for a while.

### SearXNG

[SearXNG](https://docs.searxng.org) is a free metasearch engine: it queries
other search engines and merges their results.

#### An instance you run

hodis looks for one at `http://localhost:8888/`. For any other address, use
`hodis setup` or the config file:

```toml
[searxng]
url = "https://searxng.example.org/"
```

The instance must serve JSON, which public instances often refuse: `json` has to
be listed under `search.formats` in its `settings.yml`.

#### hodis's own instance

hodis can install and run SearXNG itself, which `hodis setup` offers to do.

- The first time, it downloads a Python interpreter and SearXNG, about 90 MiB,
  into its [data directory](#files), where they take up about 280 MiB.
- The instance listens on 127.0.0.1 only and keeps running between searches.
- If it has stopped, the next search starts it again.

`embedded` in the config file says when hodis uses it, once SearXNG is the
provider chosen:

| `embedded` | hodis searches                                          | Set by                          |
| ---------- | ------------------------------------------------------- | ------------------------------- |
| `true`     | The instance at `url`, or its own if nothing answers    | Default, with no `url` set      |
| `false`    | Only the instance at `url`                              | Default, with `url` set         |
| `"only"`   | Always its own, even if something else answers at `url` | `hodis setup`, when you pick it |

### Serper

[Serper](https://serper.dev) serves Google results through an API, with a key
from your account there. hodis keeps the key in the system keychain and never
writes it to a file:

```sh
hodis key set serper                     # prompts for the key, or reads stdin
hodis key delete serper                  # removes the stored key
hodis --provider serper rust borrow checker
```

Where there is no keychain, set `SERPER_API_KEY` instead. When it is set, it
takes precedence over a stored key.

### Several at once

```sh
hodis --provider searxng,serper rust borrow checker
```

Naming several providers searches them all at once and merges what they find.
`provider = ["searxng", "serper"]` in the config file does the same for every
search.

- A page that more than one provider ranks well comes first, and each page is
  listed once.
- The answer, if any, comes from the first provider named that has one.
- A provider that fails, or that has not answered when `timeout` runs out, is
  left out with a warning. Only a search that every provider fails is an error.

## AI agents

`hodis mcp` is an [MCP](https://modelcontextprotocol.io) server on standard
input and output. Its one tool, `web_search`, takes a `query` and an optional
`page`, and returns the [JSON document](#json-output).

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
searches use the default, and `--provider searxng,serper` gives them both.
Agents share the cache with your own searches.

## Configuration

The config file is `~/.config/hodis/config.toml` on Linux and macOS and
`%APPDATA%\hodis\config.toml` on Windows; `hodis --help` names it. Every setting
is optional, and flags override the file. These are the defaults:

```toml
# provider: unset, SearXNG when it answers at its url and DuckDuckGo otherwise;
# or "searxng", "duckduckgo", "serper", or a list such as ["searxng", "serper"]
format = "pretty"      # pretty, json, markdown, plain or urls
color = "auto"         # auto, always or never
timeout = 10           # seconds a search may take before late providers are left out

[searxng]
url = "http://localhost:8888/"
embedded = true        # when to use hodis's own: true, false or "only"; see Providers

[duckduckgo]
url = "https://html.duckduckgo.com/" # only a proxy in front of DuckDuckGo needs another

[serper]
url = "https://google.serper.dev/"   # only a proxy in front of Serper needs another

[cache]
ttl = 3600             # seconds results are reused; 0 turns the cache off
```

hodis warns about keys it does not know, such as a misspelled one. `hodis setup`
keeps your comments and other settings, and if the file is a symlink, it writes
to the file the link points at, so the config can live in a dotfiles folder.

These environment variables also change what hodis does:

| Variable          | Effect                                                    |
| ----------------- | --------------------------------------------------------- |
| `HODIS_CONFIG`    | Reads this config file, which must exist                  |
| `HODIS_DATA_DIR`  | Keeps hodis's own SearXNG here                            |
| `HODIS_CACHE_DIR` | Keeps cached results here                                 |
| `SERPER_API_KEY`  | Gives the Serper key, taking precedence over a stored one |
| `NO_COLOR`        | Turns styling off when `--color` is `auto`                |

## JSON output

`--format json` prints a document for scripts. Its shape changes only with its
`schema_version`: the major part goes up when a field is removed, renamed or
retyped, and the minor part when one is added.

```json
{
  "schema_version": "2.0.0",
  "query": "capital of norway",
  "answer": {
    "url": null,
    "content": "Oslo",
    "provider": "serper"
  },
  "results": [
    {
      "title": "Oslo - Wikipedia",
      "url": "https://en.wikipedia.org/wiki/Oslo",
      "snippet": "Oslo is the capital and largest city of Norway.",
      "image": null,
      "source": "google",
      "provider": "serper"
    }
  ],
  "providers": ["serper", "searxng"],
  "diagnostics": {
    "failed_providers": [],
    "unresponsive_engines": [{ "engine": "brave", "reason": "too many requests" }],
    "skipped_results": 0
  }
}
```

| Field         | Meaning                                                                                                                                             |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| `answer`      | A direct answer, such as a fact or a definition, or `null` if there is none                                                                         |
| `source`      | The engine behind a result                                                                                                                          |
| `provider`    | The provider that returned a result                                                                                                                 |
| `providers`   | The providers that answered                                                                                                                         |
| `diagnostics` | The providers that failed or ran out of time, the engines that did not answer, and how many results were dropped for lacking a title or a valid URL |

## Files

| What            | Linux and macOS               | Windows                       | Moved by          |
| --------------- | ----------------------------- | ----------------------------- | ----------------- |
| Config file     | `~/.config/hodis/config.toml` | `%APPDATA%\hodis\config.toml` | `HODIS_CONFIG`    |
| Data directory  | `~/.local/share/hodis`        | `%APPDATA%\hodis`             | `HODIS_DATA_DIR`  |
| Cache directory | `~/.cache/hodis`              | `%LOCALAPPDATA%\hodis`        | `HODIS_CACHE_DIR` |

The data directory holds hodis's own SearXNG, if it has set one up. To stop the
instance, end the process whose ID is in `engine/searxng.pid` there. On Linux or
macOS:

```sh
kill "$(cat ~/.local/share/hodis/engine/searxng.pid)"
```

Once it has stopped, deleting the data directory removes it altogether.

Cached results live in `searches` under the cache directory. Deleting it costs
nothing but the speed of the next few searches.

## Troubleshooting

| Problem                                | What to do                                                                                                |
| -------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Setting up SearXNG failed              | Run `hodis setup` to try again. The full output is in `engine/setup.log` in the [data directory](#files). |
| DuckDuckGo asks for a CAPTCHA          | It took the search for a bot's. Search again later, or through SearXNG.                                   |
| A SearXNG instance refuses searches    | List `json` under `search.formats` in its `settings.yml`.                                                 |
| The Serper key is not kept             | There is no system keychain to keep it in. Set `SERPER_API_KEY` instead.                                  |
| An agent's first search times out      | Setting up hodis's own SearXNG can take longer than a client waits. Run one search in a terminal first.   |
| `y` does not copy a link               | Not every terminal allows OSC 52. In tmux, `set -g set-clipboard on` passes it on.                        |
| macOS refuses to run a downloaded copy | hodis is not signed. `xattr -d com.apple.quarantine hodis` clears it; the install script avoids this.     |

## License

[MIT](LICENSE)
