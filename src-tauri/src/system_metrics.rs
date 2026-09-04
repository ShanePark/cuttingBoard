use crate::models::SystemMetrics;
use std::time::Instant;
use sysinfo::{MemoryRefreshKind, System, MINIMUM_CPU_UPDATE_INTERVAL};

#[derive(Default)]
pub(crate) struct SystemMetricsState {
    system: System,
    last_cpu_sample: Option<Instant>,
}

impl SystemMetricsState {
    pub(crate) fn refresh(&mut self) -> SystemMetrics {
        self.system.refresh_cpu_usage();
        self.system
            .refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());

        let now = Instant::now();
        let cpu_percent = match self.last_cpu_sample {
            Some(previous) if now.duration_since(previous) >= MINIMUM_CPU_UPDATE_INTERVAL => {
                self.last_cpu_sample = Some(now);
                average_cpu_percent(&self.system)
            }
            Some(_) => None,
            None => {
                self.last_cpu_sample = Some(now);
                None
            }
        };

        SystemMetrics {
            cpu_percent,
            memory_percent: percentage(self.system.used_memory(), self.system.total_memory()),
        }
    }
}

fn average_cpu_percent(system: &System) -> Option<f32> {
    let cpus = system.cpus();
    if cpus.is_empty() {
        return None;
    }

    let total = cpus.iter().map(|cpu| cpu.cpu_usage()).sum::<f32>();
    Some((total / cpus.len() as f32).clamp(0.0, 100.0))
}

fn percentage(used: u64, total: u64) -> Option<f32> {
    if total == 0 {
        return None;
    }

    Some(((used as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_handles_missing_memory_and_clamps_invalid_values() {
        assert_eq!(percentage(0, 0), None);
        assert_eq!(percentage(50, 100), Some(50.0));
        assert_eq!(percentage(150, 100), Some(100.0));
    }

    #[test]
    fn first_cpu_sample_is_unavailable() {
        let mut state = SystemMetricsState::default();

        assert_eq!(state.refresh().cpu_percent, None);
    }
}
