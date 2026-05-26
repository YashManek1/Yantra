//! # forge-router: Prompt Translation
//!
//! Converts Yantra's provider-neutral completion schema to and from provider
//! request and response shapes. The translator keeps all provider-specific
//! JSON structure isolated from routing and agent code.
//!
//! ## Input
//! - `CompletionRequest` and provider response JSON values
//!
//! ## Output
//! - Anthropic, OpenAI-compatible, and Ollama request/response JSON values
//!
//! ## Related
//! - `forge-router::provider` — defines the neutral completion schema
//! - `forge-router::providers` — sends translated requests to providers

use serde_json::{json, Value};

use crate::error::ProviderError;
use crate::provider::{
    CompletionRequest, CompletionResponse, FinishReason, Message, MessageRole, ToolCall, ToolSpec,
};

/// Translator between provider-neutral and provider-specific prompt schemas.
#[derive(Debug, Clone, Copy, Default)]
pub struct PromptTranslator;

impl PromptTranslator {
    /// Converts a completion request to Anthropic chat JSON.
    pub fn to_anthropic(request: &CompletionRequest, model: &str) -> Value {
        let system_message = request
            .messages
            .iter()
            .find(|message| message.role == MessageRole::System)
            .map(|message| message.content.clone());
        let messages = request
            .messages
            .iter()
            .filter(|message| message.role != MessageRole::System)
            .map(|message| {
                json!({
                    "role": role_to_anthropic(message.role),
                    "content": message.content,
                })
            })
            .collect::<Vec<_>>();

        json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(1024),
            "temperature": stable_temperature(request.temperature),
            "system": system_message,
            "tools": request.tools.as_ref().map(|tools| tools.iter().map(tool_to_anthropic).collect::<Vec<_>>()),
            "stop_sequences": request.stop_sequences,
        })
    }

    /// Converts a completion request to OpenAI-compatible chat JSON.
    pub fn to_openai(request: &CompletionRequest, model: &str) -> Value {
        json!({
            "model": model,
            "messages": request.messages.iter().map(message_to_openai).collect::<Vec<_>>(),
            "max_tokens": request.max_tokens,
            "temperature": stable_temperature(request.temperature),
            "tools": request.tools.as_ref().map(|tools| tools.iter().map(tool_to_openai).collect::<Vec<_>>()),
            "stop": request.stop_sequences,
        })
    }

    /// Converts a completion request to Ollama chat JSON.
    pub fn to_ollama(request: &CompletionRequest, model: &str) -> Value {
        json!({
            "model": model,
            "messages": request.messages.iter().map(message_to_ollama).collect::<Vec<_>>(),
            "stream": false,
            "options": {
                "temperature": stable_temperature(request.temperature),
                "num_predict": request.max_tokens,
                "stop": request.stop_sequences,
            },
            "tools": request.tools.as_ref().map(|tools| tools.iter().map(tool_to_openai).collect::<Vec<_>>()),
        })
    }

    /// Converts an Anthropic response JSON value to a neutral response.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::InvalidResponse` if the provider response does
    /// not contain the expected fields.
    pub fn from_anthropic(response: &Value) -> Result<CompletionResponse, ProviderError> {
        let content = response
            .get("content")
            .and_then(Value::as_array)
            .map(|content_blocks| {
                content_blocks
                    .iter()
                    .filter_map(|block| block.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        let usage = response.get("usage").unwrap_or(&Value::Null);

        Ok(CompletionResponse {
            content,
            tool_calls: Vec::new(),
            tokens_in: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            tokens_out: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            finish_reason: finish_reason_from_text(
                response.get("stop_reason").and_then(Value::as_str),
            ),
        })
    }

    /// Converts an OpenAI-compatible response JSON value to a neutral response.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::InvalidResponse` if the provider response does
    /// not contain the expected fields.
    pub fn from_openai(response: &Value) -> Result<CompletionResponse, ProviderError> {
        let choice = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| ProviderError::InvalidResponse("missing choices[0]".to_string()))?;
        let message = choice.get("message").unwrap_or(&Value::Null);
        let usage = response.get("usage").unwrap_or(&Value::Null);

        Ok(CompletionResponse {
            content: message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool_calls: parse_openai_tool_calls(message.get("tool_calls")),
            tokens_in: usage
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            tokens_out: usage
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            finish_reason: finish_reason_from_text(
                choice.get("finish_reason").and_then(Value::as_str),
            ),
        })
    }

    /// Converts an Ollama response JSON value to a neutral response.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::InvalidResponse` if the provider response does
    /// not contain the expected fields.
    pub fn from_ollama(response: &Value) -> Result<CompletionResponse, ProviderError> {
        let message = response.get("message").unwrap_or(&Value::Null);

        Ok(CompletionResponse {
            content: message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool_calls: parse_openai_tool_calls(message.get("tool_calls")),
            tokens_in: response
                .get("prompt_eval_count")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            tokens_out: response
                .get("eval_count")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            finish_reason: if response
                .get("done")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                FinishReason::Stop
            } else {
                FinishReason::Other("incomplete".to_string())
            },
        })
    }
}

fn message_to_openai(message: &Message) -> Value {
    json!({
        "role": role_to_openai(message.role),
        "content": message.content,
        "tool_calls": if message.tool_calls.is_empty() {
            None
        } else {
            Some(message.tool_calls.iter().map(tool_call_to_openai).collect::<Vec<_>>())
        },
    })
}

fn message_to_ollama(message: &Message) -> Value {
    json!({
        "role": role_to_openai(message.role),
        "content": message.content,
        "tool_calls": if message.tool_calls.is_empty() {
            None
        } else {
            Some(message.tool_calls.iter().map(tool_call_to_openai).collect::<Vec<_>>())
        },
    })
}

fn tool_to_openai(tool_spec: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool_spec.name,
            "description": tool_spec.description,
            "parameters": tool_spec.parameters_json_schema,
        },
    })
}

fn tool_to_anthropic(tool_spec: &ToolSpec) -> Value {
    json!({
        "name": tool_spec.name,
        "description": tool_spec.description,
        "input_schema": tool_spec.parameters_json_schema,
    })
}

fn tool_call_to_openai(tool_call: &ToolCall) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.arguments,
        },
    })
}

fn parse_openai_tool_calls(tool_calls: Option<&Value>) -> Vec<ToolCall> {
    tool_calls
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let function = call.get("function")?;
                    Some(ToolCall {
                        id: call
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        name: function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        arguments: function
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn finish_reason_from_text(reason: Option<&str>) -> FinishReason {
    match reason.unwrap_or("stop") {
        "stop" | "end_turn" => FinishReason::Stop,
        "length" | "max_tokens" => FinishReason::Length,
        "tool_calls" | "tool_use" => FinishReason::ToolCalls,
        other => FinishReason::Other(other.to_string()),
    }
}

fn role_to_openai(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn role_to_anthropic(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn stable_temperature(temperature: f32) -> f64 {
    let temperature = f64::from(temperature);
    (temperature * 1_000.0).round() / 1_000.0
}
