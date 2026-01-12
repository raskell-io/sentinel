//! Metrics export for Protocol v2.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metrics report from an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsReport {
    pub agent_id: String,
    pub timestamp_ms: u64,
    pub interval_ms: u64,
    #[serde(default)]
    pub counters: Vec<CounterMetric>,
    #[serde(default)]
    pub gauges: Vec<GaugeMetric>,
    #[serde(default)]
    pub histograms: Vec<HistogramMetric>,
}

impl MetricsReport {
    pub fn new(agent_id: impl Into<String>, interval_ms: u64) -> Self {
        Self {
            agent_id: agent_id.into(),
            timestamp_ms: now_ms(),
            interval_ms,
            counters: Vec::new(),
            gauges: Vec::new(),
            histograms: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.counters.is_empty() && self.gauges.is_empty() && self.histograms.is_empty()
    }
}

/// A counter metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterMetric {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub value: u64,
}

impl CounterMetric {
    pub fn new(name: impl Into<String>, value: u64) -> Self {
        Self { name: name.into(), help: None, labels: HashMap::new(), value }
    }
}

/// A gauge metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeMetric {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub value: f64,
}

impl GaugeMetric {
    pub fn new(name: impl Into<String>, value: f64) -> Self {
        Self { name: name.into(), help: None, labels: HashMap::new(), value }
    }
}

/// A histogram metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramMetric {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    pub sum: f64,
    pub count: u64,
    pub buckets: Vec<HistogramBucket>,
}

/// A histogram bucket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramBucket {
    #[serde(serialize_with = "serialize_le", deserialize_with = "deserialize_le")]
    pub le: f64,
    pub count: u64,
}

impl HistogramBucket {
    pub fn new(le: f64) -> Self { Self { le, count: 0 } }
    pub fn infinity() -> Self { Self { le: f64::INFINITY, count: 0 } }
}

fn serialize_le<S>(le: &f64, serializer: S) -> Result<S::Ok, S::Error>
where S: serde::Serializer {
    if le.is_infinite() { serializer.serialize_str("+Inf") } else { serializer.serialize_f64(*le) }
}

fn deserialize_le<'de, D>(deserializer: D) -> Result<f64, D::Error>
where D: serde::Deserializer<'de> {
    use serde::de::{self, Visitor};
    struct LeVisitor;
    impl<'de> Visitor<'de> for LeVisitor {
        type Value = f64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("float or +Inf") }
        fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> { Ok(v) }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> { Ok(v as f64) }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> { Ok(v as f64) }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v == "+Inf" || v == "Inf" { Ok(f64::INFINITY) } else { v.parse().map_err(de::Error::custom) }
        }
    }
    deserializer.deserialize_any(LeVisitor)
}

/// Standard metric names.
pub mod standard {
    pub const REQUESTS_TOTAL: &str = "agent_requests_total";
    pub const REQUESTS_BLOCKED_TOTAL: &str = "agent_requests_blocked_total";
    pub const REQUESTS_DURATION_SECONDS: &str = "agent_requests_duration_seconds";
    pub const ERRORS_TOTAL: &str = "agent_errors_total";
    pub const IN_FLIGHT_REQUESTS: &str = "agent_in_flight_requests";
    pub const QUEUE_DEPTH: &str = "agent_queue_depth";
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_report() {
        let report = MetricsReport::new("test-agent", 10_000);
        assert!(report.is_empty());
    }

    #[test]
    fn test_counter_metric() {
        let counter = CounterMetric::new("test_counter", 100);
        assert_eq!(counter.value, 100);
    }

    #[test]
    fn test_histogram_bucket_infinity() {
        let bucket = HistogramBucket::infinity();
        assert!(bucket.le.is_infinite());

        let json = serde_json::to_string(&bucket).unwrap();
        assert!(json.contains("+Inf"));

        let parsed: HistogramBucket = serde_json::from_str(&json).unwrap();
        assert!(parsed.le.is_infinite());
    }
}
