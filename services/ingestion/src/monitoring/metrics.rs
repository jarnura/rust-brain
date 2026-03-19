use prometheus::{
    self, Encoder, GaugeVec, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};

/// Holds all Prometheus metrics for the ingestion pipeline.
pub struct MetricsRegistry {
    registry: Registry,
    pub stage_duration: HistogramVec,
    pub items_processed: IntCounterVec,
    pub errors: IntCounterVec,
    pub memory_pressure: GaugeVec,
    pub circuit_breaker_state: GaugeVec,
    pub stuck_warnings: IntCounterVec,
    pub degradation_tier: IntGauge,
}

impl MetricsRegistry {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let stage_duration = HistogramVec::new(
            prometheus::HistogramOpts::new(
                "ingestion_stage_duration_seconds",
                "Duration of each ingestion pipeline stage in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0]),
            &["stage"],
        )?;

        let items_processed = IntCounterVec::new(
            Opts::new(
                "ingestion_items_processed_total",
                "Total number of items processed per stage",
            ),
            &["stage"],
        )?;

        let errors = IntCounterVec::new(
            Opts::new(
                "ingestion_errors_total",
                "Total number of errors per stage and error type",
            ),
            &["stage", "error_type"],
        )?;

        let memory_pressure = GaugeVec::new(
            Opts::new(
                "ingestion_memory_pressure",
                "Current memory pressure level",
            ),
            &["level"],
        )?;

        let circuit_breaker_state = GaugeVec::new(
            Opts::new(
                "ingestion_circuit_breaker_state",
                "Circuit breaker state per service (0=closed, 1=half-open, 2=open)",
            ),
            &["service"],
        )?;

        let stuck_warnings = IntCounterVec::new(
            Opts::new(
                "ingestion_stuck_warnings_total",
                "Total stuck-detection warnings per stage",
            ),
            &["stage"],
        )?;

        let degradation_tier = IntGauge::new(
            "ingestion_degradation_tier",
            "Current degradation tier (0=normal, higher=more degraded)",
        )?;

        registry.register(Box::new(stage_duration.clone()))?;
        registry.register(Box::new(items_processed.clone()))?;
        registry.register(Box::new(errors.clone()))?;
        registry.register(Box::new(memory_pressure.clone()))?;
        registry.register(Box::new(circuit_breaker_state.clone()))?;
        registry.register(Box::new(stuck_warnings.clone()))?;
        registry.register(Box::new(degradation_tier.clone()))?;

        Ok(Self {
            registry,
            stage_duration,
            items_processed,
            errors,
            memory_pressure,
            circuit_breaker_state,
            stuck_warnings,
            degradation_tier,
        })
    }

    /// Gather all metrics and encode them in Prometheus text exposition format.
    pub fn gather(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .expect("encoding metrics should not fail");
        String::from_utf8(buffer).expect("Prometheus text format is valid UTF-8")
    }
}
