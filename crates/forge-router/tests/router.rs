use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use yantra_core::{ModelTier, TaskClass};
use yantra_router::{
    CompletionRequest, FinishReason, GitHubModelsProvider, Message, MessageRole, ModelProvider,
    OllamaProvider, OpenRouterProvider, PromptTranslator, ProviderStatus, Router, RoutingPolicy,
    TaskDescription, ToolCall, ToolSpec,
};

fn sample_request() -> CompletionRequest {
    CompletionRequest {
        messages: vec![
            Message {
                role: MessageRole::System,
                content: "Follow source truth.".to_string(),
                tool_calls: Vec::new(),
            },
            Message {
                role: MessageRole::User,
                content: "Implement the router.".to_string(),
                tool_calls: Vec::new(),
            },
        ],
        max_tokens: Some(256),
        temperature: 0.2,
        tools: Some(vec![ToolSpec {
            name: "read_file".to_string(),
            description: "Read a file.".to_string(),
            parameters_json_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }]),
        stop_sequences: vec!["STOP".to_string()],
    }
}

#[test]
fn translator_openai_round_trip_matches_golden_bytes() {
    let request = sample_request();
    let openai_request = PromptTranslator::to_openai(&request, "openrouter/auto");
    let golden_request = json!({
        "max_tokens": 256,
        "messages": [
            {
                "content": "Follow source truth.",
                "role": "system",
                "tool_calls": null
            },
            {
                "content": "Implement the router.",
                "role": "user",
                "tool_calls": null
            }
        ],
        "model": "openrouter/auto",
        "stop": ["STOP"],
        "temperature": 0.2,
        "tools": [
            {
                "function": {
                    "description": "Read a file.",
                    "name": "read_file",
                    "parameters": {
                        "properties": {
                            "path": { "type": "string" }
                        },
                        "required": ["path"],
                        "type": "object"
                    }
                },
                "type": "function"
            }
        ]
    });

    assert_eq!(
        serde_json::to_string(&openai_request).unwrap(),
        serde_json::to_string(&golden_request).unwrap()
    );

    let openai_response = json!({
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "content": "Done.",
                "tool_calls": [{
                    "id": "call-1",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"Cargo.toml\"}"
                    }
                }]
            }
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5
        }
    });
    let translated_response = PromptTranslator::from_openai(&openai_response).unwrap();

    assert_eq!(translated_response.content, "Done.");
    assert_eq!(translated_response.tokens_in, 10);
    assert_eq!(translated_response.tokens_out, 5);
    assert_eq!(translated_response.finish_reason, FinishReason::ToolCalls);
    assert_eq!(
        translated_response.tool_calls,
        vec![ToolCall {
            id: "call-1".to_string(),
            name: "read_file".to_string(),
            arguments: "{\"path\":\"Cargo.toml\"}".to_string(),
        }]
    );
}

#[test]
fn translator_anthropic_and_ollama_round_trip() {
    let request = sample_request();
    let anthropic_request = PromptTranslator::to_anthropic(&request, "claude-3-5-sonnet");
    let ollama_request = PromptTranslator::to_ollama(&request, "qwen2.5-coder:7b");

    assert_eq!(anthropic_request["model"], "claude-3-5-sonnet");
    assert_eq!(ollama_request["model"], "qwen2.5-coder:7b");

    let anthropic_response = json!({
        "content": [{ "type": "text", "text": "Anthropic response." }],
        "usage": { "input_tokens": 7, "output_tokens": 3 },
        "stop_reason": "end_turn"
    });
    let ollama_response = json!({
        "message": { "content": "Ollama response." },
        "prompt_eval_count": 8,
        "eval_count": 4,
        "done": true
    });

    assert_eq!(
        PromptTranslator::from_anthropic(&anthropic_response)
            .unwrap()
            .content,
        "Anthropic response."
    );
    assert_eq!(
        PromptTranslator::from_ollama(&ollama_response)
            .unwrap()
            .content,
        "Ollama response."
    );
}

#[test]
fn routing_classifier_matches_task_fixtures() {
    let policy = RoutingPolicy::default();
    let fixtures = vec![
        (TaskClass::BugFix, 100, 0, false, false, ModelTier::Tier0),
        (TaskClass::Chore, 1_999, 2, false, false, ModelTier::Tier0),
        (TaskClass::Style, 2_000, 2, false, false, ModelTier::Tier1),
        (
            TaskClass::Exploration,
            100,
            3,
            false,
            false,
            ModelTier::Tier1,
        ),
        (TaskClass::NewFeature, 100, 0, true, false, ModelTier::Tier3),
        (TaskClass::Migration, 100, 0, false, false, ModelTier::Tier3),
        (
            TaskClass::Integration,
            100,
            0,
            false,
            false,
            ModelTier::Tier3,
        ),
        (TaskClass::Refactor, 100, 0, false, true, ModelTier::Tier2),
        (TaskClass::Docstring, 10, 0, false, false, ModelTier::Tier0),
        (TaskClass::BugFix, 3_000, 0, false, false, ModelTier::Tier1),
        (TaskClass::BugFix, 1_000, 5, false, false, ModelTier::Tier1),
        (
            TaskClass::NewFeature,
            1_000,
            1,
            false,
            false,
            ModelTier::Tier0,
        ),
        (
            TaskClass::NewFeature,
            5_000,
            1,
            false,
            false,
            ModelTier::Tier1,
        ),
        (
            TaskClass::Refactor,
            5_000,
            1,
            false,
            false,
            ModelTier::Tier1,
        ),
        (TaskClass::Refactor, 5_000, 5, false, true, ModelTier::Tier2),
        (
            TaskClass::Migration,
            10_000,
            10,
            true,
            true,
            ModelTier::Tier3,
        ),
        (TaskClass::Integration, 10, 0, true, false, ModelTier::Tier3),
        (TaskClass::Style, 10, 4, false, false, ModelTier::Tier1),
        (
            TaskClass::Exploration,
            1_999,
            2,
            false,
            true,
            ModelTier::Tier0,
        ),
        (TaskClass::Chore, 2_001, 2, false, false, ModelTier::Tier1),
    ];

    for (fixture_index, fixture) in fixtures.into_iter().enumerate() {
        let task = TaskDescription {
            description: format!("fixture {fixture_index}"),
            class: fixture.0,
            tokens_estimated: fixture.1,
            tool_calls_predicted: fixture.2,
            touches_sacred_files: fixture.3,
            multi_file: fixture.4,
        };
        assert_eq!(policy.classify(&task), fixture.5, "fixture {fixture_index}");
    }
}

#[tokio::test]
async fn ollama_provider_sends_expected_request_shape() {
    let server = MockServer::spawn(
        "POST",
        "/api/chat",
        json!({
            "message": { "content": "ok" },
            "prompt_eval_count": 4,
            "eval_count": 2,
            "done": true
        }),
    )
    .await;
    let provider = OllamaProvider::with_endpoint("qwen2.5-coder:7b", server.url());

    let response = provider.complete(sample_request()).await.unwrap();
    let request_body = server.request_body().await;

    assert_eq!(response.content, "ok");
    assert_eq!(request_body["model"], "qwen2.5-coder:7b");
    assert_eq!(request_body["stream"], false);
    assert_eq!(request_body["messages"][1]["role"], "user");
}

#[tokio::test]
async fn openrouter_provider_sends_expected_request_shape() {
    let server = MockServer::spawn(
        "POST",
        "/",
        json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "content": "ok" }
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }),
    )
    .await;
    let provider = OpenRouterProvider::new("test-key", "openrouter/auto", server.url());

    let response = provider.complete(sample_request()).await.unwrap();
    let request_body = server.request_body().await;

    assert_eq!(response.content, "ok");
    assert_eq!(request_body["model"], "openrouter/auto");
    assert_eq!(request_body["messages"][0]["role"], "system");
}

#[tokio::test]
async fn github_models_provider_sends_expected_request_shape() {
    let server = MockServer::spawn(
        "POST",
        "/",
        json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "content": "ok" }
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
        }),
    )
    .await;
    let provider = GitHubModelsProvider::new("test-token", "gpt-4o-mini", server.url());

    let response = provider.complete(sample_request()).await.unwrap();
    let request_body = server.request_body().await;

    assert_eq!(response.content, "ok");
    assert_eq!(request_body["model"], "gpt-4o-mini");
    assert_eq!(request_body["temperature"], 0.2);
}

#[tokio::test]
async fn openrouter_status_parses_rate_limit_header() {
    let server = MockServer::spawn_with_status("HEAD", "/", 429, Some(("retry-after", "60"))).await;
    let provider = OpenRouterProvider::new("test-key", "openrouter/auto", server.url());

    assert!(matches!(
        provider.status().await,
        ProviderStatus::RateLimited { .. }
    ));
}

#[tokio::test]
async fn router_selects_provider_for_required_tier() {
    let provider: Arc<dyn ModelProvider> = Arc::new(OllamaProvider::new("qwen2.5-coder:7b"));
    let router = Router::new(RoutingPolicy::default(), vec![provider]);
    let task = TaskDescription {
        description: "small bug fix".to_string(),
        class: TaskClass::BugFix,
        tokens_estimated: 100,
        tool_calls_predicted: 1,
        touches_sacred_files: false,
        multi_file: false,
    };

    let selected_provider = router.route_task(&task, sample_request()).await.unwrap();

    assert_eq!(selected_provider.tier(), ModelTier::Tier0);
}

#[cfg(feature = "live-tests")]
#[tokio::test]
async fn live_ollama_call_if_running() {
    let provider = OllamaProvider::new("qwen2.5-coder:7b");
    if !matches!(provider.status().await, ProviderStatus::Available) {
        return;
    }

    let response = provider.complete(sample_request()).await.unwrap();

    assert!(!response.content.is_empty() || !response.tool_calls.is_empty());
}

struct MockServer {
    url: String,
    request_receiver: tokio::sync::oneshot::Receiver<Value>,
}

impl MockServer {
    async fn spawn(method: &'static str, path: &'static str, response_body: Value) -> Self {
        Self::spawn_with_status_and_body(method, path, 200, None, Some(response_body)).await
    }

    async fn spawn_with_status(
        method: &'static str,
        path: &'static str,
        status: u16,
        header: Option<(&'static str, &'static str)>,
    ) -> Self {
        Self::spawn_with_status_and_body(method, path, status, header, None).await
    }

    async fn spawn_with_status_and_body(
        expected_method: &'static str,
        expected_path: &'static str,
        status: u16,
        header: Option<(&'static str, &'static str)>,
        response_body: Option<Value>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_sender, request_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0; 8192];
            let bytes_read = stream.read(&mut buffer).await.unwrap();
            let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
            let request_line = request_text.lines().next().unwrap_or_default().to_string();
            assert!(request_line.starts_with(&format!("{expected_method} {expected_path}")));
            let request_body = request_text
                .split("\r\n\r\n")
                .nth(1)
                .filter(|body| !body.is_empty())
                .and_then(|body| serde_json::from_str(body).ok())
                .unwrap_or(Value::Null);
            let _send_result = request_sender.send(request_body);

            let serialized_response = response_body
                .map(|body| serde_json::to_string(&body).unwrap())
                .unwrap_or_default();
            let header_line = header
                .map(|(header_name, header_value)| format!("{header_name}: {header_value}\r\n"))
                .unwrap_or_default();
            let status_text = if status == 200 { "OK" } else { "ERROR" };
            let response = format!(
                "HTTP/1.1 {status} {status_text}\r\nContent-Type: application/json\r\n{header_line}Content-Length: {}\r\n\r\n{}",
                serialized_response.len(),
                serialized_response
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        Self {
            url: format!("http://{address}"),
            request_receiver,
        }
    }

    fn url(&self) -> String {
        self.url.clone()
    }

    async fn request_body(self) -> Value {
        self.request_receiver.await.unwrap()
    }
}
