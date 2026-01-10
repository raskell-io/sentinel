//! Tiktoken integration for accurate token counting.
//!
//! Provides a cached tokenizer manager that:
//! - Caches BPE instances for different encodings (avoid recreation overhead)
//! - Maps model names to the correct tokenizer encoding
//! - Extracts and tokenizes just the message content from chat completions
//!
//! # Encodings
//!
//! | Encoding | Models |
//! |----------|--------|
//! | `o200k_base` | GPT-4o, GPT-4o-mini |
//! | `cl100k_base` | GPT-4, GPT-4-turbo, GPT-3.5-turbo, text-embedding-* |
//! | `p50k_base` | Codex, text-davinci-003 |
//!
//! # Usage
//!
//! ```ignore
//! let manager = TiktokenManager::new();
//! let tokens = manager.count_tokens("gpt-4", "Hello, world!");
//! let request_tokens = manager.count_chat_request(body, Some("gpt-4o"));
//! ```

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace, warn};

#[cfg(feature = "tiktoken")]
use tiktoken_rs::{cl100k_base, o200k_base, p50k_base, CoreBPE};

/// Tiktoken encoding types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TiktokenEncoding {
    /// GPT-4o, GPT-4o-mini (newest)
    O200kBase,
    /// GPT-4, GPT-4-turbo, GPT-3.5-turbo
    Cl100kBase,
    /// Codex, text-davinci-003
    P50kBase,
}

impl TiktokenEncoding {
    /// Get the encoding name for logging
    pub fn name(&self) -> &'static str {
        match self {
            Self::O200kBase => "o200k_base",
            Self::Cl100kBase => "cl100k_base",
            Self::P50kBase => "p50k_base",
        }
    }
}

/// Global tiktoken manager instance
static TIKTOKEN_MANAGER: Lazy<TiktokenManager> = Lazy::new(TiktokenManager::new);

/// Get the global tiktoken manager
pub fn tiktoken_manager() -> &'static TiktokenManager {
    &TIKTOKEN_MANAGER
}

/// Manages cached tiktoken BPE instances for different encodings.
///
/// Thread-safe and lazily initialized - encodings are only loaded when first used.
pub struct TiktokenManager {
    #[cfg(feature = "tiktoken")]
    encodings: RwLock<HashMap<TiktokenEncoding, Arc<CoreBPE>>>,
    #[cfg(not(feature = "tiktoken"))]
    _marker: std::marker::PhantomData<()>,
}

impl TiktokenManager {
    /// Create a new tiktoken manager
    pub fn new() -> Self {
        #[cfg(feature = "tiktoken")]
        {
            Self {
                encodings: RwLock::new(HashMap::new()),
            }
        }
        #[cfg(not(feature = "tiktoken"))]
        {
            Self {
                _marker: std::marker::PhantomData,
            }
        }
    }

    /// Get the appropriate encoding for a model name
    pub fn encoding_for_model(&self, model: &str) -> TiktokenEncoding {
        let model_lower = model.to_lowercase();

        // GPT-4o family uses o200k_base
        if model_lower.contains("gpt-4o") || model_lower.contains("gpt4o") {
            return TiktokenEncoding::O200kBase;
        }

        // GPT-4 and GPT-3.5-turbo use cl100k_base
        if model_lower.contains("gpt-4")
            || model_lower.contains("gpt-3.5")
            || model_lower.contains("gpt-35")
            || model_lower.contains("text-embedding")
            || model_lower.contains("claude") // Claude approximation
        {
            return TiktokenEncoding::Cl100kBase;
        }

        // Codex and older models use p50k_base
        if model_lower.contains("code-")
            || model_lower.contains("codex")
            || model_lower.contains("text-davinci-003")
            || model_lower.contains("text-davinci-002")
        {
            return TiktokenEncoding::P50kBase;
        }

        // Default to cl100k_base (most common)
        TiktokenEncoding::Cl100kBase
    }

    /// Count tokens in text using the appropriate encoding for the model
    #[cfg(feature = "tiktoken")]
    pub fn count_tokens(&self, model: Option<&str>, text: &str) -> u64 {
        let encoding = model
            .map(|m| self.encoding_for_model(m))
            .unwrap_or(TiktokenEncoding::Cl100kBase);

        self.count_tokens_with_encoding(encoding, text)
    }

    #[cfg(not(feature = "tiktoken"))]
    pub fn count_tokens(&self, _model: Option<&str>, text: &str) -> u64 {
        // Fallback to character-based estimation
        (text.chars().count() / 4).max(1) as u64
    }

    /// Count tokens using a specific encoding
    #[cfg(feature = "tiktoken")]
    pub fn count_tokens_with_encoding(&self, encoding: TiktokenEncoding, text: &str) -> u64 {
        match self.get_or_create_bpe(encoding) {
            Some(bpe) => {
                let tokens = bpe.encode_with_special_tokens(text);
                tokens.len() as u64
            }
            None => {
                // Fallback to character estimation
                (text.chars().count() / 4).max(1) as u64
            }
        }
    }

    #[cfg(not(feature = "tiktoken"))]
    pub fn count_tokens_with_encoding(&self, _encoding: TiktokenEncoding, text: &str) -> u64 {
        (text.chars().count() / 4).max(1) as u64
    }

    /// Count tokens in a chat completion request body
    ///
    /// Parses the JSON and extracts message content for accurate token counting.
    /// Returns estimated tokens including overhead for message formatting.
    pub fn count_chat_request(&self, body: &[u8], model: Option<&str>) -> u64 {
        // Try to parse as JSON
        let json: Value = match serde_json::from_slice(body) {
            Ok(v) => v,
            Err(_) => {
                // If not valid JSON, count the whole body as text
                let text = String::from_utf8_lossy(body);
                return self.count_tokens(model, &text);
            }
        };

        // Extract model from body if not provided
        let model_name = model.or_else(|| json.get("model").and_then(|m| m.as_str()));

        // Extract messages array
        let messages = match json.get("messages").and_then(|m| m.as_array()) {
            Some(msgs) => msgs,
            None => {
                // Not a chat completion request, try other formats
                return self.count_non_chat_request(&json, model_name);
            }
        };

        // Count tokens in messages
        let mut total_tokens: u64 = 0;

        // Per-message overhead (role, separators, etc.)
        // OpenAI uses ~4 tokens overhead per message
        const MESSAGE_OVERHEAD: u64 = 4;

        for message in messages {
            // Add message overhead
            total_tokens += MESSAGE_OVERHEAD;

            // Count role tokens (typically 1 token)
            if let Some(role) = message.get("role").and_then(|r| r.as_str()) {
                total_tokens += self.count_tokens(model_name, role);
            }

            // Count content tokens
            if let Some(content) = message.get("content") {
                match content {
                    Value::String(text) => {
                        total_tokens += self.count_tokens(model_name, text);
                    }
                    Value::Array(parts) => {
                        // Multi-modal content (text + images)
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                total_tokens += self.count_tokens(model_name, text);
                            }
                            // Image tokens are estimated separately (not text)
                            if part.get("image_url").is_some() {
                                // Rough estimate: 85 tokens for low detail, 765 for high detail
                                total_tokens += 170; // Medium estimate
                            }
                        }
                    }
                    _ => {}
                }
            }

            // Count name tokens if present
            if let Some(name) = message.get("name").and_then(|n| n.as_str()) {
                total_tokens += self.count_tokens(model_name, name);
            }

            // Count tool calls if present
            if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
                for tool_call in tool_calls {
                    if let Some(function) = tool_call.get("function") {
                        if let Some(name) = function.get("name").and_then(|n| n.as_str()) {
                            total_tokens += self.count_tokens(model_name, name);
                        }
                        if let Some(args) = function.get("arguments").and_then(|a| a.as_str()) {
                            total_tokens += self.count_tokens(model_name, args);
                        }
                    }
                }
            }
        }

        // Add conversation overhead (typically 3 tokens)
        total_tokens += 3;

        // Account for max_tokens in response (estimate output)
        if let Some(max_tokens) = json.get("max_tokens").and_then(|m| m.as_u64()) {
            // Add estimated output tokens (assume ~50% utilization)
            total_tokens += max_tokens / 2;
        }

        trace!(
            message_count = messages.len(),
            total_tokens = total_tokens,
            model = ?model_name,
            "Counted tokens in chat request"
        );

        total_tokens
    }

    /// Count tokens for non-chat requests (completions, embeddings)
    fn count_non_chat_request(&self, json: &Value, model: Option<&str>) -> u64 {
        let mut total_tokens: u64 = 0;

        // Legacy completions API: { "prompt": "..." }
        if let Some(prompt) = json.get("prompt") {
            match prompt {
                Value::String(text) => {
                    total_tokens += self.count_tokens(model, text);
                }
                Value::Array(prompts) => {
                    for p in prompts {
                        if let Some(text) = p.as_str() {
                            total_tokens += self.count_tokens(model, text);
                        }
                    }
                }
                _ => {}
            }
        }

        // Embeddings API: { "input": "..." }
        if let Some(input) = json.get("input") {
            match input {
                Value::String(text) => {
                    total_tokens += self.count_tokens(model, text);
                }
                Value::Array(inputs) => {
                    for i in inputs {
                        if let Some(text) = i.as_str() {
                            total_tokens += self.count_tokens(model, text);
                        }
                    }
                }
                _ => {}
            }
        }

        // If still zero, count the whole body
        if total_tokens == 0 {
            let body_text = json.to_string();
            total_tokens = self.count_tokens(model, &body_text);
        }

        total_tokens
    }

    /// Get or create a BPE instance for the given encoding
    #[cfg(feature = "tiktoken")]
    fn get_or_create_bpe(&self, encoding: TiktokenEncoding) -> Option<Arc<CoreBPE>> {
        // Try read lock first
        {
            let cache = self.encodings.read();
            if let Some(bpe) = cache.get(&encoding) {
                return Some(Arc::clone(bpe));
            }
        }

        // Need to create - acquire write lock
        let mut cache = self.encodings.write();

        // Double-check after acquiring write lock
        if let Some(bpe) = cache.get(&encoding) {
            return Some(Arc::clone(bpe));
        }

        // Create the encoding
        let bpe = match encoding {
            TiktokenEncoding::O200kBase => {
                debug!(encoding = "o200k_base", "Initializing tiktoken encoding");
                o200k_base().ok()
            }
            TiktokenEncoding::Cl100kBase => {
                debug!(encoding = "cl100k_base", "Initializing tiktoken encoding");
                cl100k_base().ok()
            }
            TiktokenEncoding::P50kBase => {
                debug!(encoding = "p50k_base", "Initializing tiktoken encoding");
                p50k_base().ok()
            }
        };

        match bpe {
            Some(bpe) => {
                let arc_bpe = Arc::new(bpe);
                cache.insert(encoding, Arc::clone(&arc_bpe));
                Some(arc_bpe)
            }
            None => {
                warn!(
                    encoding = encoding.name(),
                    "Failed to initialize tiktoken encoding"
                );
                None
            }
        }
    }

    /// Check if tiktoken is available (feature enabled)
    pub fn is_available(&self) -> bool {
        cfg!(feature = "tiktoken")
    }
}

impl Default for TiktokenManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoding_for_model() {
        let manager = TiktokenManager::new();

        // GPT-4o uses o200k_base
        assert_eq!(
            manager.encoding_for_model("gpt-4o"),
            TiktokenEncoding::O200kBase
        );
        assert_eq!(
            manager.encoding_for_model("gpt-4o-mini"),
            TiktokenEncoding::O200kBase
        );

        // GPT-4 uses cl100k_base
        assert_eq!(
            manager.encoding_for_model("gpt-4"),
            TiktokenEncoding::Cl100kBase
        );
        assert_eq!(
            manager.encoding_for_model("gpt-4-turbo"),
            TiktokenEncoding::Cl100kBase
        );
        assert_eq!(
            manager.encoding_for_model("gpt-3.5-turbo"),
            TiktokenEncoding::Cl100kBase
        );

        // Claude uses cl100k approximation
        assert_eq!(
            manager.encoding_for_model("claude-3-opus"),
            TiktokenEncoding::Cl100kBase
        );

        // Codex uses p50k_base
        assert_eq!(
            manager.encoding_for_model("code-davinci-002"),
            TiktokenEncoding::P50kBase
        );

        // Unknown defaults to cl100k_base
        assert_eq!(
            manager.encoding_for_model("unknown-model"),
            TiktokenEncoding::Cl100kBase
        );
    }

    #[test]
    fn test_count_tokens_basic() {
        let manager = TiktokenManager::new();

        // Basic text counting
        let tokens = manager.count_tokens(Some("gpt-4"), "Hello, world!");
        assert!(tokens > 0);

        // Without model (uses default)
        let tokens = manager.count_tokens(None, "Hello, world!");
        assert!(tokens > 0);
    }

    #[test]
    fn test_count_chat_request() {
        let manager = TiktokenManager::new();

        let body = br#"{
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello!"}
            ]
        }"#;

        let tokens = manager.count_chat_request(body, None);
        assert!(tokens > 0);
        // Should be roughly: system message (~10) + user message (~5) + overhead (~10)
        assert!(tokens >= 10);
    }

    #[test]
    fn test_count_chat_request_with_tools() {
        let manager = TiktokenManager::new();

        let body = br#"{
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "What's the weather?"},
                {"role": "assistant", "tool_calls": [
                    {"function": {"name": "get_weather", "arguments": "{\"city\": \"NYC\"}"}}
                ]}
            ]
        }"#;

        let tokens = manager.count_chat_request(body, None);
        assert!(tokens > 0);
    }

    #[test]
    fn test_count_embeddings_request() {
        let manager = TiktokenManager::new();

        let body = br#"{
            "model": "text-embedding-ada-002",
            "input": "Hello, world!"
        }"#;

        let tokens = manager.count_chat_request(body, None);
        assert!(tokens > 0);
    }

    #[test]
    fn test_count_invalid_json() {
        let manager = TiktokenManager::new();

        let body = b"not valid json at all";
        let tokens = manager.count_chat_request(body, Some("gpt-4"));
        // Should fall back to text counting
        assert!(tokens > 0);
    }

    #[test]
    #[cfg(feature = "tiktoken")]
    fn test_tiktoken_accurate_hello_world() {
        let manager = TiktokenManager::new();

        // "Hello world" is typically 2 tokens with cl100k_base
        let tokens = manager.count_tokens_with_encoding(TiktokenEncoding::Cl100kBase, "Hello world");
        assert_eq!(tokens, 2);
    }

    #[test]
    #[cfg(feature = "tiktoken")]
    fn test_tiktoken_caching() {
        let manager = TiktokenManager::new();

        // First call creates the encoding
        let tokens1 = manager.count_tokens(Some("gpt-4"), "Test message");
        // Second call should use cached encoding
        let tokens2 = manager.count_tokens(Some("gpt-4"), "Test message");

        assert_eq!(tokens1, tokens2);
    }
}
