//! The MCP server answering a client in the same process, over an in-memory
//! pipe, with a mock SearXNG instance behind it.

use std::time::Duration;

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
};
use serde_json::{Value, json};
use url::Url;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use hodis::{
    Provider,
    config::Config,
    mcp::Server,
    output::{OutputFormat, render},
    providers::searxng::SearXNGProvider,
};

const HEALTHY_BODY: &str = include_str!("fixtures/healthy.json");

type Client = RunningService<RoleClient, ()>;

/// A client of a server whose searches go to `searxng`.
async fn client_for(searxng: &MockServer) -> anyhow::Result<Client> {
    let config = Config::parse(&format!("[searxng]\nurl = \"{}\"\n", searxng.uri()))?.config;
    let server = Server::new(config, vec![Provider::SearXNG])?;
    let (server_end, client_end) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        server.serve(server_end).await?.waiting().await?;
        anyhow::Ok(())
    });
    Ok(().serve(client_end).await?)
}

async fn web_search(client: &Client, arguments: Value) -> anyhow::Result<CallToolResult> {
    let Value::Object(arguments) = arguments else {
        anyhow::bail!("tool arguments are an object");
    };
    let params = CallToolRequestParams::new("web_search").with_arguments(arguments);
    Ok(client.call_tool(params).await?)
}

/// Every `format` keyword in `schema`, at any depth.
fn formats(schema: &Value) -> Vec<&str> {
    match schema {
        Value::Object(map) => map
            .iter()
            .flat_map(|(key, value)| {
                let own = if key == "format" {
                    value.as_str()
                } else {
                    None
                };
                own.into_iter().chain(formats(value))
            })
            .collect(),
        Value::Array(items) => items.iter().flat_map(formats).collect(),
        _ => Vec::new(),
    }
}

fn text(result: &CallToolResult) -> &str {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map_or("", |content| content.text.as_str())
}

async fn mount_healthy(searxng: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("q", "dns lookup"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .mount(searxng)
        .await;
}

#[tokio::test]
async fn the_server_introduces_itself_as_hodis() -> anyhow::Result<()> {
    let client = client_for(&MockServer::start().await).await?;

    let info = client.peer_info().expect("the handshake has happened");
    let server = info.server_info.as_ref().expect("the server named itself");
    assert_eq!(server.name, "hodis");
    assert_eq!(server.version, env!("CARGO_PKG_VERSION"));
    Ok(())
}

#[tokio::test]
async fn web_search_is_the_one_tool_and_only_reads() -> anyhow::Result<()> {
    let client = client_for(&MockServer::start().await).await?;

    let tools = client.list_all_tools().await?;
    assert_eq!(tools.len(), 1);
    let tool = &tools[0];
    assert_eq!(tool.name, "web_search");
    let annotations = tool.annotations.as_ref().expect("the tool is annotated");
    assert_eq!(annotations.read_only_hint, Some(true));
    assert_eq!(annotations.open_world_hint, Some(true));
    assert_eq!(tool.input_schema.get("required"), Some(&json!(["query"])));
    assert!(tool.output_schema.is_some(), "results are structured");
    Ok(())
}

#[tokio::test]
async fn the_tool_schemas_use_only_formats_json_schema_defines() -> anyhow::Result<()> {
    // Validators in clients warn about any other, such as schemars' `uint`.
    const DEFINED: [&str; 19] = [
        "date-time",
        "date",
        "time",
        "duration",
        "email",
        "idn-email",
        "hostname",
        "idn-hostname",
        "ipv4",
        "ipv6",
        "uri",
        "uri-reference",
        "iri",
        "iri-reference",
        "uuid",
        "uri-template",
        "json-pointer",
        "relative-json-pointer",
        "regex",
    ];
    let client = client_for(&MockServer::start().await).await?;

    let tools = client.list_all_tools().await?;
    let input = Value::Object(tools[0].input_schema.as_ref().clone());
    let output = tools[0].output_schema.as_ref().expect("an output schema");
    let output = Value::Object(output.as_ref().clone());

    for format in formats(&input).into_iter().chain(formats(&output)) {
        assert!(DEFINED.contains(&format), "{format}");
    }
    Ok(())
}

#[tokio::test]
async fn a_search_returns_the_json_formats_document() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    mount_healthy(&searxng).await;
    let client = client_for(&searxng).await?;

    let result = web_search(&client, json!({"query": "dns lookup"})).await?;

    let response = SearXNGProvider {
        base_url: Url::parse(&searxng.uri())?,
    }
    .parse_body("dns lookup", HEALTHY_BODY)?;
    let mut document = Vec::new();
    render(&response, OutputFormat::Json, &mut document)?;
    let document: Value = serde_json::from_slice(&document)?;

    assert_eq!(result.is_error, Some(false));
    assert_eq!(result.structured_content.as_ref(), Some(&document));
    // Clients that ignore structured results read the same document as text.
    assert_eq!(serde_json::from_str::<Value>(text(&result))?, document);
    Ok(())
}

#[tokio::test]
async fn results_conform_to_the_advertised_output_schema() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    mount_healthy(&searxng).await;
    let client = client_for(&searxng).await?;

    let tools = client.list_all_tools().await?;
    let schema = tools[0].output_schema.as_ref().expect("an output schema");
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(&Value::Object(schema.as_ref().clone()))?;
    let result = web_search(&client, json!({"query": "dns lookup"})).await?;

    let content = result.structured_content.expect("a structured result");
    let errors: Vec<_> = validator
        .iter_errors(&content)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
    Ok(())
}

#[tokio::test]
async fn the_page_reaches_the_provider() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .and(query_param("pageno", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(1)
        .mount(&searxng)
        .await;
    let client = client_for(&searxng).await?;

    let result = web_search(&client, json!({"query": "dns lookup", "page": 2})).await?;

    assert_eq!(result.is_error, Some(false), "{}", text(&result));
    Ok(())
}

#[tokio::test]
async fn a_failed_search_is_an_error_the_agent_can_read() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&searxng)
        .await;
    let client = client_for(&searxng).await?;

    let result = web_search(&client, json!({"query": "dns lookup"})).await?;

    assert_eq!(result.is_error, Some(true));
    assert!(text(&result).contains("500"), "{}", text(&result));
    Ok(())
}

#[tokio::test]
async fn an_empty_query_is_refused_without_a_search() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(0)
        .mount(&searxng)
        .await;
    let client = client_for(&searxng).await?;

    let result = web_search(&client, json!({"query": "  "})).await?;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(text(&result), "the query is empty");
    Ok(())
}

#[tokio::test]
async fn page_zero_is_refused_without_a_search() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HEALTHY_BODY))
        .expect(0)
        .mount(&searxng)
        .await;
    let client = client_for(&searxng).await?;

    let result = web_search(&client, json!({"query": "dns lookup", "page": 0})).await?;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(text(&result), "pages are numbered from 1");
    Ok(())
}

#[tokio::test]
async fn searches_made_at_once_are_all_answered() -> anyhow::Result<()> {
    let searxng = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(HEALTHY_BODY)
                .set_delay(Duration::from_millis(100)),
        )
        .expect(3)
        .mount(&searxng)
        .await;
    let client = client_for(&searxng).await?;

    let (first, second, third) = tokio::join!(
        web_search(&client, json!({"query": "dns lookup"})),
        web_search(&client, json!({"query": "dns lookup", "page": 2})),
        web_search(&client, json!({"query": "dns lookup", "page": 3})),
    );

    for result in [first?, second?, third?] {
        assert_eq!(result.is_error, Some(false), "{}", text(&result));
    }
    Ok(())
}
