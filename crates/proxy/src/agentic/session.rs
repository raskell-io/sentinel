//! The gateway's MCP session token.
//!
//! When one endpoint fronts several MCP upstreams, a single client session spans
//! several server sessions: Streamable HTTP has each upstream issue its own
//! `Mcp-Session-Id` on `initialize`, and a client that calls tools from two
//! upstreams is holding two of them without knowing it. Something has to
//! remember which is which.
//!
//! That mapping lives in the token itself rather than in the gateway. The
//! `Mcp-Session-Id` handed to the client *is* an encrypted envelope containing
//! the upstream session IDs, decrypted on each request. No shared store, nothing
//! lost on restart, and two gateway instances can serve the same client without
//! the load balancer being told to pin it — which matters, because the
//! alternative is exporting an affinity requirement to the operator's
//! infrastructure and calling it a design.
//!
//! # Two things this must get right
//!
//! **Encrypted, not signed.** [`crate::upstream::sticky_session`] signs a target
//! index with HMAC and hands it to the client, which is fine: the index is not a
//! secret. Upstream session IDs are, so this uses AEAD. A client must not be
//! able to read them, and must not be able to forge one.
//!
//! **The key comes from configuration.** `StickySessionRuntime` generates its
//! HMAC key with `rand::rng()` at construction, so it rotates on every rebuild
//! and silently invalidates every cookie. Repeating that here would drop live
//! MCP sessions on every config reload, and an MCP session is work in flight
//! rather than a warm cache. A configured key is also what lets two instances
//! decrypt each other's tokens.

use std::collections::BTreeMap;

use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// Largest token this will produce or accept, in encoded bytes.
///
/// Headers have limits and MCP does not say what they are, so the bound is
/// ours: exceeded, the session is refused with an error naming the cause rather
/// than truncated into a token that fails to decrypt later. At roughly 60 bytes
/// per upstream entry this is comfortably past any realistic fan-out.
pub const MAX_TOKEN_BYTES: usize = 4096;

/// Why a token could not be produced or read.
#[derive(Debug, PartialEq, Eq)]
pub enum SessionError {
    /// The configured key was not 32 bytes.
    KeyLength(usize),
    /// The token did not decode, decrypt, or authenticate.
    ///
    /// Deliberately one variant. Distinguishing "wrong key" from "tampered" from
    /// "truncated" in a value the client supplies tells an attacker which of
    /// those they achieved.
    Invalid,
    /// The token would exceed [`MAX_TOKEN_BYTES`].
    TooLarge(usize),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyLength(n) => {
                write!(f, "session key must be 32 bytes, got {n}")
            }
            Self::Invalid => write!(f, "session token is not valid"),
            Self::TooLarge(n) => write!(
                f,
                "session token would be {n} bytes, over the {MAX_TOKEN_BYTES} limit"
            ),
        }
    }
}

impl std::error::Error for SessionError {}

/// Encrypts and decrypts the gateway's session tokens.
///
/// Its [`std::fmt::Debug`] deliberately shows nothing about the key. A struct
/// holding key material is exactly the sort of thing that ends up in a log line
/// via `{:?}` on some enclosing type, and the default derive would put it there.
pub struct SessionCodec {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl std::fmt::Debug for SessionCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionCodec { key: <redacted> }")
    }
}

impl SessionCodec {
    /// Build a codec from a 32-byte key.
    pub fn new(key: &[u8]) -> Result<Self, SessionError> {
        if key.len() != 32 {
            return Err(SessionError::KeyLength(key.len()));
        }
        let unbound = UnboundKey::new(&AES_256_GCM, key).map_err(|_| SessionError::Invalid)?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
            rng: SystemRandom::new(),
        })
    }

    /// Encrypt a map of upstream name to that upstream's session ID.
    ///
    /// The map is ordered, so the same set of sessions always serialises the
    /// same way; the nonce is fresh per call, so the same map still produces a
    /// different token each time.
    pub fn encode(&self, sessions: &BTreeMap<String, String>) -> Result<String, SessionError> {
        let plaintext = serialize(sessions);

        let mut nonce_bytes = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce_bytes)
            .map_err(|_| SessionError::Invalid)?;

        let mut buf = plaintext.into_bytes();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::empty(),
                &mut buf,
            )
            .map_err(|_| SessionError::Invalid)?;

        let mut envelope = Vec::with_capacity(NONCE_LEN + buf.len());
        envelope.extend_from_slice(&nonce_bytes);
        envelope.append(&mut buf);

        let token = URL_SAFE_NO_PAD.encode(&envelope);
        if token.len() > MAX_TOKEN_BYTES {
            return Err(SessionError::TooLarge(token.len()));
        }
        Ok(token)
    }

    /// Decrypt a token back into its upstream sessions.
    pub fn decode(&self, token: &str) -> Result<BTreeMap<String, String>, SessionError> {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(SessionError::TooLarge(token.len()));
        }

        let envelope = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|_| SessionError::Invalid)?;
        if envelope.len() <= NONCE_LEN {
            return Err(SessionError::Invalid);
        }

        let (nonce_bytes, ciphertext) = envelope.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce_bytes.try_into().map_err(|_| SessionError::Invalid)?;

        let mut buf = ciphertext.to_vec();
        let plaintext = self
            .key
            .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut buf)
            .map_err(|_| SessionError::Invalid)?;

        deserialize(std::str::from_utf8(plaintext).map_err(|_| SessionError::Invalid)?)
    }
}

/// `name=session` pairs joined by newlines.
///
/// Newline and `=` cannot appear in either field -- upstream names come from
/// configuration and are validated there, and a session ID carrying one is
/// refused rather than encoded, since a token that round-trips into a different
/// map than it started as is worse than a rejected request.
fn serialize(sessions: &BTreeMap<String, String>) -> String {
    sessions
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn deserialize(s: &str) -> Result<BTreeMap<String, String>, SessionError> {
    if s.is_empty() {
        return Ok(BTreeMap::new());
    }
    s.lines()
        .map(|line| {
            line.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .ok_or(SessionError::Invalid)
        })
        .collect()
}

/// Whether a session ID can be carried in a token.
///
/// Checked before encoding rather than after, because the failure it prevents
/// is silent: a value containing the separator would decode into a different
/// map than it was built from.
pub fn is_encodable(session_id: &str) -> bool {
    !session_id.contains('\n') && !session_id.contains('=')
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8; 32] = b"0123456789abcdef0123456789abcdef";

    fn codec() -> SessionCodec {
        SessionCodec::new(KEY).expect("valid key")
    }

    fn sessions(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_token_round_trips() {
        let s = sessions(&[("docs", "sess-a1"), ("warehouse", "sess-b7")]);
        let c = codec();
        assert_eq!(
            c.decode(&c.encode(&s).expect("encoded")).expect("decoded"),
            s
        );
    }

    #[test]
    fn an_empty_map_round_trips() {
        let c = codec();
        let empty = BTreeMap::new();
        assert_eq!(
            c.decode(&c.encode(&empty).expect("encoded"))
                .expect("decoded"),
            empty
        );
    }

    /// The whole point of choosing AEAD over a signature: a client holding the
    /// token must not learn which upstreams exist or what their session IDs are.
    #[test]
    fn upstream_session_ids_are_not_readable_from_the_token() {
        let s = sessions(&[("warehouse", "super-secret-session")]);
        let token = codec().encode(&s).expect("encoded");
        assert!(!token.contains("warehouse"));
        assert!(!token.contains("super-secret-session"));
        let decoded = URL_SAFE_NO_PAD.decode(&token).expect("base64");
        let as_text = String::from_utf8_lossy(&decoded);
        assert!(!as_text.contains("warehouse"));
        assert!(!as_text.contains("super-secret-session"));
    }

    #[test]
    fn a_tampered_token_is_refused() {
        let c = codec();
        let token = c
            .encode(&sessions(&[("docs", "sess-a1")]))
            .expect("encoded");

        // Flip a bit in the ciphertext, keeping it valid base64.
        let mut raw = URL_SAFE_NO_PAD.decode(&token).expect("base64");
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let tampered = URL_SAFE_NO_PAD.encode(&raw);

        assert_eq!(c.decode(&tampered), Err(SessionError::Invalid));
    }

    /// A token from another deployment must not decrypt here.
    #[test]
    fn a_token_from_a_different_key_is_refused() {
        let mine = codec();
        let theirs = SessionCodec::new(b"ffffffffffffffffffffffffffffffff").expect("valid key");
        let token = theirs
            .encode(&sessions(&[("docs", "sess-a1")]))
            .expect("encoded");
        assert_eq!(mine.decode(&token), Err(SessionError::Invalid));
    }

    /// Two instances sharing a configured key can read each other's tokens.
    /// This is what makes horizontal scaling work without load-balancer
    /// affinity, and is the reason the key must not be per-process.
    #[test]
    fn two_codecs_with_the_same_key_interoperate() {
        let a = codec();
        let b = codec();
        let s = sessions(&[("docs", "sess-a1")]);
        assert_eq!(
            b.decode(&a.encode(&s).expect("encoded")).expect("decoded"),
            s
        );
    }

    /// A fresh nonce per call, so identical sessions do not produce a stable
    /// token a client could recognise or correlate across sessions.
    #[test]
    fn the_same_map_encodes_differently_each_time() {
        let c = codec();
        let s = sessions(&[("docs", "sess-a1")]);
        assert_ne!(
            c.encode(&s).expect("encoded"),
            c.encode(&s).expect("encoded")
        );
    }

    #[test]
    fn garbage_is_refused_rather_than_panicking() {
        let c = codec();
        for bad in ["", "not base64 !!!", "aaaa", &"A".repeat(100)] {
            assert!(c.decode(bad).is_err(), "accepted {bad:?}");
        }
    }

    /// The error names the length it got, because "invalid key" on a config
    /// value sends an operator looking in the wrong place.
    #[test]
    fn a_key_of_the_wrong_length_is_rejected_with_its_length() {
        assert!(matches!(
            SessionCodec::new(b"short").err(),
            Some(SessionError::KeyLength(5))
        ));
        assert!(matches!(
            SessionCodec::new(&[0u8; 16]).err(),
            Some(SessionError::KeyLength(16))
        ));
        assert!(SessionCodec::new(&[0u8; 32]).is_ok());
    }

    #[test]
    fn an_oversized_token_is_refused_both_ways() {
        let c = codec();
        let huge: BTreeMap<String, String> = (0..200)
            .map(|i| {
                (
                    format!("upstream-{i:03}"),
                    format!("session-{i:03}-{}", "x".repeat(40)),
                )
            })
            .collect();
        assert!(matches!(c.encode(&huge), Err(SessionError::TooLarge(_))));
        assert!(matches!(
            c.decode(&"A".repeat(MAX_TOKEN_BYTES + 1)),
            Err(SessionError::TooLarge(_))
        ));
    }

    /// A session ID carrying the separator would decode into a different map
    /// than it was built from, so it is rejected before it can.
    #[test]
    fn session_ids_carrying_the_separator_are_not_encodable() {
        assert!(is_encodable("sess-a1"));
        assert!(!is_encodable("sess\na1"));
        assert!(!is_encodable("sess=a1"));
    }
}
