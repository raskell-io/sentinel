//! Native awareness of agentic protocols.
//!
//! MCP and A2A are both JSON-RPC 2.0 over HTTP POST, with SSE for streaming
//! responses. They differ in what a proxy can trust:
//!
//! - **MCP** mirrors `method` and the tool/resource name into HTTP headers
//!   expressly so intermediaries can route without reading the body. That is
//!   convenient and, taken at face value, unsound — see [`mcp`].
//! - **A2A** carries no such mirror, so the method is only in the body. There
//!   is nothing to disagree with, and correspondingly nothing to spoof.
//!
//! # Why this is in the proxy rather than an agent
//!
//! Zentinel's architecture puts complex logic in external agents, and that is
//! the right default. These checks are the exception for one reason: they are
//! decisions about whether to forward a request at all, made from data the
//! proxy already has in hand. Round-tripping every tool call to an agent to ask
//! "does this header match this body" would add a network hop to a string
//! comparison, and would put the enforcement of a security control behind a
//! failure mode (agent unavailable) that the control exists to survive.
//!
//! Anything that needs to reason about tool *arguments* — prompt injection in a
//! parameter, PII in a payload — remains agent work. This module decides
//! whether a request is coherent and permitted; agents decide whether its
//! contents are safe.

pub mod a2a;
pub mod jsonrpc;
pub mod mcp;
