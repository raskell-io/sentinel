//! Namespacing tool names across several MCP upstreams.
//!
//! When one endpoint fronts several upstreams, two of them will eventually both
//! offer `search`. The client sees one merged list, so the collision has to be
//! resolved before it gets there.
//!
//! Each upstream declares a prefix in configuration and every one of its tools
//! is exposed as `prefix.tool`. The alternative — prefixing only when a
//! collision actually occurs — was rejected deliberately: a tool's name is what
//! the model reasons about and stores, and having it change because someone
//! added an unrelated upstream that happened to collide is the kind of implicit
//! behaviour this proxy is supposed to refuse. Deriving the prefix from the
//! upstream's own name was rejected for the same reason in reverse: names
//! chosen for infrastructure reasons read poorly to a model, and renaming an
//! upstream would rename its tools.
//!
//! The mapping is total and reversible, which is what lets the request path
//! route by prefix and the response path present the merged view:
//!
//! ```text
//! upstream "docs"      prefix "docs"       search  ->  docs.search
//! upstream "warehouse" prefix "warehouse"  query   ->  warehouse.query
//! ```

use serde_json::Value;

/// Separator between a prefix and the tool's own name.
///
/// A dot reads naturally to a model and is accepted by MCP, which places no
/// constraint on tool-name characters beyond them being a string.
pub const SEPARATOR: char = '.';

/// Apply a prefix to a name.
pub fn qualify(prefix: &str, name: &str) -> String {
    format!("{prefix}{SEPARATOR}{name}")
}

/// Split a qualified name back into its prefix and the upstream's own name.
///
/// Splits on the **first** separator, so a tool whose own name contains a dot
/// survives the round trip: `docs.v2.search` belongs to `docs` and is called
/// upstream as `v2.search`.
pub fn split(qualified: &str) -> Option<(&str, &str)> {
    qualified.split_once(SEPARATOR)
}

/// The upstream a call is destined for, given the configured prefixes.
///
/// Returns `None` when the name carries no known prefix — which is a refusal,
/// not a fallback. Guessing an upstream for an unqualified name would route a
/// call somewhere the operator never said it should go.
pub fn route<'a>(qualified: &str, prefixes: &'a [String]) -> Option<(&'a str, String)> {
    let (prefix, rest) = split(qualified)?;
    prefixes
        .iter()
        .find(|p| p.as_str() == prefix)
        .map(|p| (p.as_str(), rest.to_string()))
}

/// Rewrite the `name` of each entry in one upstream's listing so it carries the
/// upstream's prefix.
///
/// **Only `name`.** [`super::listing`] identifies an entry as `name` falling
/// back to `uri`, and the fallback is deliberately not followed here: a resource
/// URI is already a globally-scoped identifier, and `docs.file:///a.txt` is not
/// a URI at all. Prefixing one would hand clients a value they cannot parse,
/// resolve or compare, in order to solve a collision that URIs mostly do not
/// have — two upstreams serving the same `file:///a.txt` is a real ambiguity but
/// a rare one, where two upstreams both offering `search` is close to certain.
///
/// So tools and prompts, which carry bare names, are namespaced; resources are
/// left alone and multiplexing them is out of scope until there is a URI-shaped
/// answer. An entry with no `name` is passed through untouched rather than
/// dropped: the enforcer would not refuse a call it cannot name either.
pub fn qualify_listing(entries: &mut [Value], prefix: &str) {
    for entry in entries {
        let Some(obj) = entry.as_object_mut() else {
            continue;
        };
        if let Some(Value::String(s)) = obj.get("name") {
            let qualified = qualify(prefix, s);
            obj.insert("name".to_string(), Value::String(qualified));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_name_round_trips_through_its_prefix() {
        let q = qualify("docs", "search");
        assert_eq!(q, "docs.search");
        assert_eq!(split(&q), Some(("docs", "search")));
    }

    /// Splitting on the first separator, so an upstream whose own tool names
    /// contain dots is not mangled on the way through.
    #[test]
    fn a_tool_name_containing_the_separator_survives() {
        let q = qualify("docs", "v2.search");
        assert_eq!(q, "docs.v2.search");
        assert_eq!(split(&q), Some(("docs", "v2.search")));
    }

    #[test]
    fn a_call_routes_to_the_upstream_its_prefix_names() {
        let p = prefixes(&["docs", "warehouse"]);
        assert_eq!(
            route("warehouse.query", &p),
            Some(("warehouse", "query".to_string()))
        );
    }

    /// An unqualified or unknown-prefix name is refused rather than guessed.
    /// Picking an upstream here would send a call somewhere no one configured.
    #[test]
    fn an_unknown_or_missing_prefix_does_not_route() {
        let p = prefixes(&["docs"]);
        assert_eq!(route("search", &p), None);
        assert_eq!(route("other.search", &p), None);
    }

    /// Two upstreams offering the same tool are distinguishable after
    /// namespacing, which is the collision this exists to resolve.
    #[test]
    fn colliding_tools_become_distinct() {
        assert_ne!(qualify("docs", "search"), qualify("wiki", "search"));
    }

    #[test]
    fn a_listing_is_qualified_by_name() {
        let mut entries: Vec<Value> =
            serde_json::from_str(r#"[{"name":"search","description":"d"},{"name":"fetch"}]"#)
                .expect("json");
        qualify_listing(&mut entries, "docs");
        assert_eq!(entries[0]["name"], "docs.search");
        assert_eq!(entries[1]["name"], "docs.fetch");
        // Everything else survives.
        assert_eq!(entries[0]["description"], "d");
    }

    /// A resource URI is left alone. Prefixing it would produce
    /// `docs.file:///a.txt`, which is not a URI and which no client could
    /// resolve or compare — a mangler rather than a namespace.
    #[test]
    fn a_resource_uri_is_not_prefixed() {
        let mut entries: Vec<Value> =
            serde_json::from_str(r#"[{"uri":"file:///a.txt"}]"#).expect("json");
        qualify_listing(&mut entries, "docs");
        assert_eq!(entries[0]["uri"], "file:///a.txt");
    }

    /// An entry carrying both is namespaced on `name` only, so its URI stays
    /// resolvable.
    #[test]
    fn an_entry_with_both_fields_keeps_its_uri() {
        let mut entries: Vec<Value> =
            serde_json::from_str(r#"[{"name":"n","uri":"file:///a.txt"}]"#).expect("json");
        qualify_listing(&mut entries, "docs");
        assert_eq!(entries[0]["name"], "docs.n");
        assert_eq!(entries[0]["uri"], "file:///a.txt");
    }

    #[test]
    fn an_unidentifiable_entry_is_left_alone() {
        let mut entries: Vec<Value> =
            serde_json::from_str(r#"[{"description":"no name"}]"#).expect("json");
        qualify_listing(&mut entries, "docs");
        assert_eq!(entries[0]["description"], "no name");
        assert!(entries[0].get("name").is_none());
    }

    /// The namespaced name is what the route's allow/deny list from #457 sees,
    /// so hiding and enforcement stay in agreement after merging.
    #[test]
    fn what_is_advertised_is_what_routes() {
        let p = prefixes(&["docs"]);
        let mut entries: Vec<Value> = serde_json::from_str(r#"[{"name":"search"}]"#).expect("json");
        qualify_listing(&mut entries, "docs");

        let advertised = entries[0]["name"].as_str().expect("name");
        assert_eq!(route(advertised, &p), Some(("docs", "search".to_string())));
    }
}
