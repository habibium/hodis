//! Web search as a tool for AI agents, served over the Model Context Protocol
//! on standard input and output.

use std::io;
use std::num::NonZeroU32;

use anstream::AutoStream;
use anyhow::Result;
use reqwest::Client;
use rmcp::{
    Json, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerConfig},
    service::ServerInitializeError,
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::cache::Cache;
use crate::config::Config;
use crate::output::Versioned;
use crate::progress::Progress;
use crate::{Providers, SearchParams, SearchResponse, providers};

/// An MCP server with one tool, `web_search`.
#[derive(Debug)]
pub struct Server {
    config: Config,
    providers: Providers,
    client: Client,
    cache: Option<Cache>,
}

// Field docs are the descriptions agents read in the tool's input schema.
#[derive(Debug, Deserialize, JsonSchema)]
struct WebSearch {
    /// What to search the web for
    query: String,
    /// The page of results to return, counting from 1
    // Unsigned rather than nonzero so that 0 reaches the tool, which answers
    // it with an error the agent can act on rather than a protocol error.
    #[schemars(range(min = 1), transform = crate::drop_format)]
    page: Option<u32>,
}

impl Server {
    /// A server searching `providers`, set up as `config` says, reusing the
    /// responses `cache` holds.
    pub fn new(config: Config, providers: Providers, cache: Option<Cache>) -> Result<Self> {
        let client = Client::builder().build()?;
        Ok(Self {
            config,
            providers,
            client,
            cache,
        })
    }

    /// Serves the client on stdin and stdout until it disconnects.
    pub async fn serve_stdio(self) -> Result<()> {
        let running = match self.serve(stdio()).await {
            Ok(running) => running,
            // Hanging up before the handshake ends things as cleanly as after.
            Err(ServerInitializeError::ConnectionClosed(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        running.waiting().await?;
        Ok(())
    }

    async fn search(&self, params: &SearchParams) -> Result<SearchResponse> {
        let mut progress = Progress::plain(AutoStream::auto(io::stderr()));
        providers::search(
            &self.providers,
            params,
            &self.config,
            &self.client,
            self.cache.as_ref(),
            &mut progress,
        )
        .await
    }
}

#[tool_router]
impl Server {
    #[tool(
        title = "Web search",
        description = "Search the web. Returns a page of results, best first, each with a title, URL and snippet, and a direct answer when there is one.",
        annotations(read_only_hint = true, open_world_hint = true)
    )]
    async fn web_search(
        &self,
        Parameters(args): Parameters<WebSearch>,
    ) -> Result<Json<Versioned<SearchResponse>>, String> {
        if args.query.trim().is_empty() {
            return Err("the query is empty".to_owned());
        }
        let page = match args.page {
            Some(page) => Some(NonZeroU32::new(page).ok_or("pages are numbered from 1")?),
            None => None,
        };
        let params = SearchParams {
            query: args.query,
            page,
        };

        match self.search(&params).await {
            Ok(response) => Ok(Json(Versioned::new(response))),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

#[allow(
    clippy::unused_async_trait_impl,
    reason = "the macro writes handlers with nothing to await as async fns"
)]
#[tool_handler]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
                .with_description(env!("CARGO_PKG_DESCRIPTION"))
                .with_website_url(env!("CARGO_PKG_REPOSITORY")),
        )
    }
}
