use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(name = "hodis", version, about = "Websearch tool for humans and AI")]
struct Cli {
    /// Engine id (ddg, gh, crates)
    #[arg(short, long, value_enum, default_value = "ddg")]
    engine: SearchEngine,

    /// Print the URL instead of opening it
    #[arg(long)]
    print_url: bool,

    /// Query terms
    #[arg(trailing_var_arg = true, required = true)]
    query: Vec<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum SearchEngine {
    Ddg,
    Google,
    Bing,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let template = match cli.engine {
        SearchEngine::Ddg => "https://duckduckgo.com/?q={query}",
        SearchEngine::Google => "https://www.google.com/search?q={query}",
        SearchEngine::Bing => "https://www.bing.com/search?q={query}",
    };

    let url = hodis::build_url(template, &cli.query.join(" "))?;

    if cli.print_url {
        println!("{url}")
    };

    Ok(())
}
