//! Semantic guardrails for inference routes.
//!
//! Provides content inspection via external agents:
//! - Prompt injection detection on requests
//! - PII detection on responses

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pingora_timeout::timeout;
use sentinel_agent_protocol::{
    GuardrailDetection, GuardrailInspectEvent, GuardrailInspectionType, GuardrailResponse,
};
use sentinel_config::{
    GuardrailAction, GuardrailFailureMode, PiiDetectionConfig, PromptInjectionConfig,
};
use tracing::{debug, trace, warn};

use crate::agents::AgentManager;

/// Result of a prompt injection check
#[derive(Debug)]
pub enum PromptInjectionResult {
    /// Content is clean (no injection detected)
    Clean,
    /// Injection detected, request should be blocked
    Blocked {
        status: u16,
        message: String,
        detections: Vec<GuardrailDetection>,
    },
    /// Injection detected but allowed (logged only)
    Detected { detections: Vec<GuardrailDetection> },
    /// Injection detected, add warning header
    Warning { detections: Vec<GuardrailDetection> },
    /// Agent error (behavior depends on failure mode)
    Error { message: String },
}

/// Result of a PII detection check
#[derive(Debug)]
pub enum PiiCheckResult {
    /// Content is clean (no PII detected)
    Clean,
    /// PII detected
    Detected {
        detections: Vec<GuardrailDetection>,
        redacted_content: Option<String>,
    },
    /// Agent error
    Error { message: String },
}

/// Guardrail processor for semantic content analysis.
///
/// Uses external agents to inspect content for security issues
/// like prompt injection and PII leakage.
pub struct GuardrailProcessor {
    agent_manager: Arc<AgentManager>,
}

impl GuardrailProcessor {
    /// Create a new guardrail processor.
    pub fn new(agent_manager: Arc<AgentManager>) -> Self {
        Self { agent_manager }
    }

    /// Check request content for prompt injection.
    ///
    /// # Arguments
    /// * `config` - Prompt injection detection configuration
    /// * `content` - Request body content to inspect
    /// * `model` - Model name if available
    /// * `route_id` - Route ID for context
    /// * `correlation_id` - Request correlation ID
    pub async fn check_prompt_injection(
        &self,
        config: &PromptInjectionConfig,
        content: &str,
        model: Option<&str>,
        route_id: Option<&str>,
        correlation_id: &str,
    ) -> PromptInjectionResult {
        if !config.enabled {
            return PromptInjectionResult::Clean;
        }

        trace!(
            correlation_id = correlation_id,
            agent = %config.agent,
            content_len = content.len(),
            "Checking content for prompt injection"
        );

        let event = GuardrailInspectEvent {
            correlation_id: correlation_id.to_string(),
            inspection_type: GuardrailInspectionType::PromptInjection,
            content: content.to_string(),
            model: model.map(String::from),
            categories: vec![],
            route_id: route_id.map(String::from),
            metadata: HashMap::new(),
        };

        let start = Instant::now();
        let timeout_duration = Duration::from_millis(config.timeout_ms);

        // Call the agent
        match timeout(
            timeout_duration,
            self.call_guardrail_agent(&config.agent, event),
        )
        .await
        {
            Ok(Ok(response)) => {
                let duration = start.elapsed();
                debug!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    detected = response.detected,
                    confidence = response.confidence,
                    detection_count = response.detections.len(),
                    duration_ms = duration.as_millis(),
                    "Prompt injection check completed"
                );

                if response.detected {
                    match config.action {
                        GuardrailAction::Block => PromptInjectionResult::Blocked {
                            status: config.block_status,
                            message: config.block_message.clone().unwrap_or_else(|| {
                                "Request blocked: potential prompt injection detected".to_string()
                            }),
                            detections: response.detections,
                        },
                        GuardrailAction::Log => PromptInjectionResult::Detected {
                            detections: response.detections,
                        },
                        GuardrailAction::Warn => PromptInjectionResult::Warning {
                            detections: response.detections,
                        },
                    }
                } else {
                    PromptInjectionResult::Clean
                }
            }
            Ok(Err(e)) => {
                warn!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    error = %e,
                    failure_mode = ?config.failure_mode,
                    "Prompt injection agent call failed"
                );

                match config.failure_mode {
                    GuardrailFailureMode::Open => PromptInjectionResult::Clean,
                    GuardrailFailureMode::Closed => PromptInjectionResult::Blocked {
                        status: 503,
                        message: "Guardrail check unavailable".to_string(),
                        detections: vec![],
                    },
                }
            }
            Err(_) => {
                warn!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    timeout_ms = config.timeout_ms,
                    failure_mode = ?config.failure_mode,
                    "Prompt injection agent call timed out"
                );

                match config.failure_mode {
                    GuardrailFailureMode::Open => PromptInjectionResult::Clean,
                    GuardrailFailureMode::Closed => PromptInjectionResult::Blocked {
                        status: 504,
                        message: "Guardrail check timed out".to_string(),
                        detections: vec![],
                    },
                }
            }
        }
    }

    /// Check response content for PII.
    ///
    /// # Arguments
    /// * `config` - PII detection configuration
    /// * `content` - Response content to inspect
    /// * `route_id` - Route ID for context
    /// * `correlation_id` - Request correlation ID
    pub async fn check_pii(
        &self,
        config: &PiiDetectionConfig,
        content: &str,
        route_id: Option<&str>,
        correlation_id: &str,
    ) -> PiiCheckResult {
        if !config.enabled {
            return PiiCheckResult::Clean;
        }

        trace!(
            correlation_id = correlation_id,
            agent = %config.agent,
            content_len = content.len(),
            categories = ?config.categories,
            "Checking response for PII"
        );

        let event = GuardrailInspectEvent {
            correlation_id: correlation_id.to_string(),
            inspection_type: GuardrailInspectionType::PiiDetection,
            content: content.to_string(),
            model: None,
            categories: config.categories.clone(),
            route_id: route_id.map(String::from),
            metadata: HashMap::new(),
        };

        let start = Instant::now();
        let timeout_duration = Duration::from_millis(config.timeout_ms);

        match timeout(
            timeout_duration,
            self.call_guardrail_agent(&config.agent, event),
        )
        .await
        {
            Ok(Ok(response)) => {
                let duration = start.elapsed();
                debug!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    detected = response.detected,
                    detection_count = response.detections.len(),
                    duration_ms = duration.as_millis(),
                    "PII check completed"
                );

                if response.detected {
                    PiiCheckResult::Detected {
                        detections: response.detections,
                        redacted_content: response.redacted_content,
                    }
                } else {
                    PiiCheckResult::Clean
                }
            }
            Ok(Err(e)) => {
                warn!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    error = %e,
                    "PII detection agent call failed"
                );

                PiiCheckResult::Error {
                    message: e.to_string(),
                }
            }
            Err(_) => {
                warn!(
                    correlation_id = correlation_id,
                    agent = %config.agent,
                    timeout_ms = config.timeout_ms,
                    "PII detection agent call timed out"
                );

                PiiCheckResult::Error {
                    message: "Agent timeout".to_string(),
                }
            }
        }
    }

    /// Call a guardrail agent with an inspection event.
    async fn call_guardrail_agent(
        &self,
        agent_name: &str,
        event: GuardrailInspectEvent,
    ) -> Result<GuardrailResponse, String> {
        // Use the agent manager to send the guardrail event
        // For now, we'll use a simple direct approach
        // The agent manager needs a method to handle GuardrailInspect events

        // This is a placeholder - the actual implementation would use
        // the agent manager's connection pool and protocol handling
        trace!(
            agent = agent_name,
            inspection_type = ?event.inspection_type,
            "Calling guardrail agent"
        );

        // For now, return a mock response until we integrate with agent manager
        // In a real implementation, this would call the agent via the manager
        Err(format!(
            "Agent '{}' not configured for guardrail inspection",
            agent_name
        ))
    }
}

/// Extract message content from an inference request body.
///
/// Attempts to parse the body as JSON and extract message content
/// from common inference API formats (OpenAI, Anthropic, etc.)
pub fn extract_inference_content(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;

    // OpenAI format: {"messages": [{"content": "..."}]}
    if let Some(messages) = json.get("messages").and_then(|m| m.as_array()) {
        let content: Vec<String> = messages
            .iter()
            .filter_map(|msg| msg.get("content").and_then(|c| c.as_str()))
            .map(String::from)
            .collect();
        if !content.is_empty() {
            return Some(content.join("\n"));
        }
    }

    // Anthropic format: {"prompt": "..."}
    if let Some(prompt) = json.get("prompt").and_then(|p| p.as_str()) {
        return Some(prompt.to_string());
    }

    // Generic: look for common content fields
    for field in &["input", "text", "query", "question"] {
        if let Some(value) = json.get(*field).and_then(|v| v.as_str()) {
            return Some(value.to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_openai_content() {
        let body = br#"{"messages": [{"role": "user", "content": "Hello world"}]}"#;
        let content = extract_inference_content(body);
        assert_eq!(content, Some("Hello world".to_string()));
    }

    #[test]
    fn test_extract_openai_multi_message() {
        let body = br#"{
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "Hello"}
            ]
        }"#;
        let content = extract_inference_content(body);
        assert_eq!(content, Some("You are helpful\nHello".to_string()));
    }

    #[test]
    fn test_extract_anthropic_content() {
        let body = br#"{"prompt": "Human: Hello\n\nAssistant:"}"#;
        let content = extract_inference_content(body);
        assert_eq!(content, Some("Human: Hello\n\nAssistant:".to_string()));
    }

    #[test]
    fn test_extract_generic_input() {
        let body = br#"{"input": "Test query"}"#;
        let content = extract_inference_content(body);
        assert_eq!(content, Some("Test query".to_string()));
    }

    #[test]
    fn test_extract_invalid_json() {
        let body = b"not json";
        let content = extract_inference_content(body);
        assert_eq!(content, None);
    }
}
