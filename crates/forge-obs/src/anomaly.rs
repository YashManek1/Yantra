//! # forge-obs: Anomaly Detection
//!
//! Detects runtime anomalies from rolling one-hour samples of error rate,
//! latency, and token throughput. The detector can run as a Tokio task and
//! emits events over an MPSC channel when a metric exceeds a z-score of 3.0.
//!
//! ## Input
//! - Periodic runtime samples captured every 30 seconds
//! - Error rate, latency, and tokens-per-minute measurements
//!
//! ## Output
//! - `AnomalyEvent` values sent to observers
//!
//! ## Related
//! - `forge-obs::watchdog` — consumes anomaly signals for safety policy

use std::collections::VecDeque;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

/// Number of samples retained for a one-hour window at 30-second intervals.
pub const ROLLING_WINDOW_SAMPLES: usize = 120;

/// Sampling interval for the background detector.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Metric that crossed the anomaly threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnomalyMetric {
    /// Error rate anomaly.
    ErrorRate,
    /// Latency anomaly.
    Latency,
    /// Token throughput anomaly.
    TokensPerMinute,
}

/// One runtime sample consumed by the detector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnomalySample {
    /// Timestamp of the sample.
    pub sampled_at: DateTime<Utc>,
    /// Error rate in the interval.
    pub error_rate: f64,
    /// Average latency in milliseconds.
    pub latency_ms: f64,
    /// Tokens produced or consumed per minute.
    pub tokens_per_minute: f64,
}

/// Event emitted when a metric crosses the anomaly threshold.
#[derive(Debug, Clone, PartialEq)]
pub struct AnomalyEvent {
    /// Metric that crossed the threshold.
    pub metric: AnomalyMetric,
    /// Z-score for the triggering sample.
    pub z_score: f64,
    /// Sample that triggered the event.
    pub sample: AnomalySample,
}

/// Rolling z-score anomaly detector.
#[derive(Debug, Default)]
pub struct AnomalyDetector {
    samples: VecDeque<AnomalySample>,
}

impl AnomalyDetector {
    /// Creates an empty anomaly detector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a sample and returns an anomaly event when one is detected.
    pub fn observe(&mut self, sample: AnomalySample) -> Option<AnomalyEvent> {
        let anomaly_event = self.detect_against_existing_window(sample);
        self.samples.push_back(sample);
        while self.samples.len() > ROLLING_WINDOW_SAMPLES {
            self.samples.pop_front();
        }
        anomaly_event
    }

    /// Runs the detector as a background Tokio task.
    pub fn spawn(
        mut sample_receiver: mpsc::Receiver<AnomalySample>,
        event_sender: mpsc::Sender<AnomalyEvent>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut detector = Self::new();
            let mut sample_interval = tokio::time::interval(SAMPLE_INTERVAL);
            loop {
                tokio::select! {
                    received_sample = sample_receiver.recv() => {
                        match received_sample {
                            Some(sample) => {
                                if let Some(anomaly_event) = detector.observe(sample) {
                                    let _send_result = event_sender.send(anomaly_event).await;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = sample_interval.tick() => {}
                }
            }
        })
    }

    fn detect_against_existing_window(&self, sample: AnomalySample) -> Option<AnomalyEvent> {
        if self.samples.len() < 30 {
            return None;
        }

        [
            (
                AnomalyMetric::ErrorRate,
                sample.error_rate,
                self.samples
                    .iter()
                    .map(|existing_sample| existing_sample.error_rate)
                    .collect::<Vec<_>>(),
            ),
            (
                AnomalyMetric::Latency,
                sample.latency_ms,
                self.samples
                    .iter()
                    .map(|existing_sample| existing_sample.latency_ms)
                    .collect::<Vec<_>>(),
            ),
            (
                AnomalyMetric::TokensPerMinute,
                sample.tokens_per_minute,
                self.samples
                    .iter()
                    .map(|existing_sample| existing_sample.tokens_per_minute)
                    .collect::<Vec<_>>(),
            ),
        ]
        .into_iter()
        .filter_map(|(metric, current_value, historical_values)| {
            z_score(current_value, &historical_values).map(|computed_z_score| AnomalyEvent {
                metric,
                z_score: computed_z_score,
                sample,
            })
        })
        .find(|event| event.z_score > 3.0)
    }
}

fn z_score(current_value: f64, historical_values: &[f64]) -> Option<f64> {
    let historical_count =
        u32::try_from(historical_values.len()).map_or(f64::from(u32::MAX), f64::from);
    if historical_count == 0.0 {
        return None;
    }

    let mean = historical_values.iter().sum::<f64>() / historical_count;
    let variance = historical_values
        .iter()
        .map(|historical_value| {
            let delta = historical_value - mean;
            delta * delta
        })
        .sum::<f64>()
        / historical_count;
    let standard_deviation = variance.sqrt();

    if standard_deviation <= f64::EPSILON {
        return None;
    }

    Some((current_value - mean) / standard_deviation)
}
