//! StatusBar.svelte / state/metrics.svelte.ts, using the same core SSH exec channel.
//! Only one visible, connected SSH tab is polled. No work runs on the GUI thread.
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use nexssh_core::{
    Core, SessionId,
    metrics::{CpuTimes, Metrics, Usage},
};
use slint::{ModelRc, VecModel};

use crate::{AppWindow, StatusMetric, i18n};

const INTERVAL: Duration = Duration::from_secs(2);
const FIRST_CPU: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(15);
const UNSUPPORTED: Duration = Duration::from_secs(60);
const STALE: Duration = Duration::from_secs(10);
const HISTORY: Duration = Duration::from_secs(60);

struct Sample {
    at: Instant,
    values: [Option<f64>; 3],
}

struct Reading {
    metrics: Metrics,
    cpu: Option<f64>,
    previous: Option<(CpuTimes, Instant)>,
    history: VecDeque<Sample>,
}

struct Completion {
    session: SessionId,
    generation: u64,
    at: Instant,
    result: Result<Metrics, String>,
}

pub struct Monitor {
    readings: BTreeMap<SessionId, Option<Reading>>,
    watching: Option<SessionId>,
    generation: u64,
    next: Instant,
    task: Option<tokio::task::JoinHandle<()>>,
    sender: mpsc::Sender<Completion>,
    receiver: mpsc::Receiver<Completion>,
    painted: Option<(Option<SessionId>, String)>,
    dirty: bool,
}

impl Default for Monitor {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            readings: BTreeMap::new(),
            watching: None,
            generation: 0,
            next: Instant::now(),
            task: None,
            sender,
            receiver,
            painted: None,
            dirty: true,
        }
    }
}

impl Monitor {
    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.watching = None;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(SessionId) -> bool) {
        self.readings.retain(|id, _| keep(*id));
    }

    pub fn tick(
        &mut self,
        core: &Arc<Core>,
        runtime: &tokio::runtime::Runtime,
        session: Option<SessionId>,
        now: Instant,
    ) {
        if session != self.watching {
            self.stop();
            self.watching = session;
            self.next = now;
        }
        while let Ok(event) = self.receiver.try_recv() {
            // A response from a hidden/closed/switched tab must not repaint another one.
            if event.generation != self.generation || self.watching != Some(event.session) {
                continue;
            }
            self.task.take();
            let delay = match event.result {
                Ok(metrics) => {
                    self.dirty = true;
                    self.record(event.session, metrics, event.at)
                }
                Err(error) => {
                    log::debug!("Server metrics: {error}");
                    RETRY
                }
            };
            self.next = event.at + delay;
        }
        if let Some(session) = session
            && self.task.is_none()
            && now >= self.next
        {
            let core = Arc::clone(core);
            let sender = self.sender.clone();
            let generation = self.generation;
            self.task = Some(runtime.spawn(async move {
                let result = nexssh_core::metrics::read(&core.sessions, session)
                    .await
                    .map_err(|e| e.to_string());
                let _ = sender.send(Completion {
                    session,
                    generation,
                    at: Instant::now(),
                    result,
                });
            }));
        }
    }

    fn record(&mut self, id: SessionId, metrics: Metrics, at: Instant) -> Duration {
        self.dirty = true;
        if metrics.is_empty() {
            self.readings.insert(id, None);
            return UNSUPPORTED;
        }
        let mut previous = self.readings.remove(&id).flatten();
        let last = previous
            .as_ref()
            .and_then(|r| r.previous)
            .filter(|(_, when)| at.saturating_duration_since(*when) < STALE);
        let cpu = metrics
            .cpu
            .and_then(|next| cpu_busy(last.map(|(times, _)| times), next));
        let delay = if metrics.cpu.is_some() && last.is_none() {
            FIRST_CPU
        } else {
            INTERVAL
        };
        let mut history = previous
            .as_mut()
            .map(|r| std::mem::take(&mut r.history))
            .unwrap_or_default();
        history.retain(|s| at.saturating_duration_since(s.at) <= HISTORY);
        history.push_back(Sample {
            at,
            values: [
                cpu,
                metrics.memory.map(used_share),
                metrics.disk.map(used_share),
            ],
        });
        let reading = Reading {
            cpu: cpu.or_else(|| previous.as_ref().and_then(|r| r.cpu)),
            previous: metrics.cpu.map(|times| (times, at)).or(last),
            metrics,
            history,
        };
        self.readings.insert(id, Some(reading));
        delay
    }

    pub fn apply(&mut self, ui: &AppWindow, session: Option<SessionId>, lang: &str) {
        if !self.dirty
            && self
                .painted
                .as_ref()
                .is_some_and(|(id, language)| *id == session && language == lang)
        {
            return;
        }
        let state = session.and_then(|id| self.readings.get(&id));
        ui.set_metrics_unavailable(matches!(state, Some(None)));
        let mut rows = vec![];
        let mut up = String::new();
        let mut up_tip = String::new();
        if let Some(Some(reading)) = state {
            let m = &reading.metrics;
            if let Some(seconds) = m.uptime {
                let time = uptime(seconds, lang);
                up = message("status.uptime", lang, &[("time", time.clone())]);
                up_tip = message("status.uptimeTip", lang, &[("time", time)]);
            }
            if m.cpu.is_some() {
                let mut tip = reading
                    .cpu
                    .map(|value| message("status.cpuTip", lang, &[("value", percent(value, lang))]))
                    .unwrap_or_default();
                if let Some(load) = m.load {
                    let load = load
                        .iter()
                        .map(|v| decimal(*v, 3, lang, false))
                        .collect::<Vec<_>>()
                        .join("  ");
                    append(
                        &mut tip,
                        &message("status.loadTip", lang, &[("load", load)]),
                    );
                }
                rows.push(meter(
                    reading,
                    0,
                    reading.cpu,
                    reading.cpu.map_or("—".into(), |v| percent(v, lang)),
                    tip,
                    lang,
                ));
            }
            if let Some(memory) = m.memory {
                let mut tip = usage_tip("status.memoryTip", memory, lang);
                if let Some(swap) = m.swap {
                    append(&mut tip, &usage_tip("status.swapTip", swap, lang));
                }
                rows.push(meter(
                    reading,
                    1,
                    Some(used_share(memory)),
                    pair(memory, lang),
                    tip,
                    lang,
                ));
            }
            if let Some(disk) = m.disk {
                rows.push(meter(
                    reading,
                    2,
                    Some(used_share(disk)),
                    pair(disk, lang),
                    usage_tip("status.diskTip", disk, lang),
                    lang,
                ));
            }
        }
        ui.set_status_uptime(up.into());
        ui.set_status_uptime_tip(up_tip.into());
        ui.set_status_metrics(ModelRc::new(VecModel::from(rows)));
        self.painted = Some((session, lang.to_owned()));
        self.dirty = false;
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop();
    }
}

fn cpu_busy(previous: Option<CpuTimes>, next: CpuTimes) -> Option<f64> {
    let previous = previous?;
    let total = next.total.checked_sub(previous.total).filter(|n| *n > 0)?;
    let idle = next.idle.checked_sub(previous.idle)?;
    Some((1.0 - idle as f64 / total as f64).clamp(0.0, 1.0))
}

fn used_share(u: Usage) -> f64 {
    let of = u.used as f64 + u.free as f64;
    if of > 0.0 {
        (u.used as f64 / of).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn message(key: &str, lang: &str, values: &[(&str, String)]) -> String {
    let mut text = i18n::tr(key, lang);
    for (name, value) in values {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text
}

fn append(text: &mut String, line: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(line);
}

fn decimal(value: f64, places: usize, lang: &str, fixed: bool) -> String {
    let text = format!("{value:.places$}");
    let text = if fixed || places == 0 {
        text
    } else {
        text.trim_end_matches('0').trim_end_matches('.').into()
    };
    if lang == "ru" {
        text.replace('.', ",")
    } else {
        text
    }
}

fn percent(share: f64, lang: &str) -> String {
    format!(
        "{}{space}%",
        (share.clamp(0.0, 1.0) * 100.0).round() as u64,
        space = if lang == "ru" { "\u{a0}" } else { "" }
    )
}

fn unit_of(bytes: u64) -> usize {
    let mut unit = 0;
    while unit < 4 && bytes as f64 >= 1000.0 * 1024.0_f64.powi(unit as i32) {
        unit += 1;
    }
    unit
}

fn in_unit(bytes: u64, unit: usize, lang: &str) -> String {
    let value = bytes as f64 / 1024.0_f64.powi(unit as i32);
    decimal(value, usize::from(unit > 0 && value < 10.0), lang, true)
}

fn size(bytes: u64, lang: &str) -> String {
    let unit = unit_of(bytes);
    format!(
        "{} {}",
        in_unit(bytes, unit, lang),
        i18n::tr(
            ["size.b", "size.kb", "size.mb", "size.gb", "size.tb"][unit],
            lang
        )
    )
}

fn pair(u: Usage, lang: &str) -> String {
    let unit = unit_of(u.total);
    format!(
        "{} / {} {}",
        in_unit(u.used, unit, lang),
        in_unit(u.total, unit, lang),
        i18n::tr(
            ["size.b", "size.kb", "size.mb", "size.gb", "size.tb"][unit],
            lang
        )
    )
}

fn usage_tip(key: &str, usage: Usage, lang: &str) -> String {
    message(
        key,
        lang,
        &[
            ("used", size(usage.used, lang)),
            ("total", size(usage.total, lang)),
            ("free", size(usage.free, lang)),
        ],
    )
}

fn uptime(seconds: u64, lang: &str) -> String {
    let days = seconds / 86400;
    let hours = seconds % 86400 / 3600;
    let minutes = seconds % 3600 / 60;
    let parts = if days > 0 {
        vec![("days", days), ("hours", hours)]
    } else if hours > 0 {
        vec![("hours", hours), ("minutes", minutes)]
    } else {
        vec![("minutes", minutes)]
    };
    parts
        .iter()
        .enumerate()
        .filter(|(i, (_, n))| *i == 0 || *n > 0)
        .map(|(_, (unit, n))| message(&format!("time.{unit}"), lang, &[("count", n.to_string())]))
        .collect::<Vec<_>>()
        .join(" ")
}

fn meter(
    reading: &Reading,
    index: usize,
    share: Option<f64>,
    value: String,
    mut detail: String,
    lang: &str,
) -> StatusMetric {
    let values: Vec<_> = reading
        .history
        .iter()
        .filter_map(|s| s.values[index])
        .collect();
    if reading.history.len() > 1 && !values.is_empty() {
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let avg = values.iter().sum::<f64>() / values.len() as f64;
        append(
            &mut detail,
            &message(
                "status.minuteTip",
                lang,
                &[
                    ("min", percent(min, lang)),
                    ("max", percent(max, lang)),
                    ("avg", percent(avg, lang)),
                ],
            ),
        );
    }
    let graph = slint::Image::load_from_svg_data(graph(&reading.history, index).as_bytes())
        .unwrap_or_default();
    StatusMetric {
        label: i18n::tr(["status.cpu", "status.memory", "status.disk"][index], lang).into(),
        value: value.into(),
        detail: detail.into(),
        level: share.map_or(0, |s| {
            if s >= 0.9 {
                2
            } else if s >= 0.75 {
                1
            } else {
                0
            }
        }),
        graph,
    }
}

fn graph(history: &VecDeque<Sample>, index: usize) -> String {
    let end = history.back().map(|s| s.at);
    let mut runs: Vec<Vec<(f64, f64)>> = vec![];
    let mut previous = None;
    let mut run = vec![];
    for sample in history {
        if (sample.values[index].is_none()
            || previous.is_some_and(|at| sample.at.saturating_duration_since(at) > INTERVAL * 3))
            && !run.is_empty()
        {
            runs.push(std::mem::take(&mut run));
        }
        previous = Some(sample.at);
        if let (Some(value), Some(end)) = (sample.values[index], end) {
            let x = 60.0
                * (1.0
                    - end.saturating_duration_since(sample.at).as_secs_f64()
                        / HISTORY.as_secs_f64());
            run.push((x.max(0.0), 1.0 + 14.0 * (1.0 - value.clamp(0.0, 1.0))));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    let mut svg = String::from(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="60" height="16" viewBox="0 0 60 16">"#,
    );
    for mut points in runs {
        if points.len() == 1 {
            let (x, y) = points[0];
            points.insert(0, ((x - 2.0).max(0.0), y));
        }
        let line = points
            .iter()
            .enumerate()
            .map(|(i, (x, y))| format!("{}{x:.1} {y:.1}", if i == 0 { "M" } else { "L" }))
            .collect::<String>();
        let x0 = points[0].0;
        let xn = points.last().unwrap().0;
        svg.push_str(&format!(r#"<path fill="white" fill-opacity="0.18" d="M{x0:.1} 16 {} L{xn:.1} 16 Z"/><path fill="none" stroke="white" stroke-width="1.25" stroke-linejoin="round" stroke-linecap="round" d="{line}"/>"#,line.replacen('M',"L",1)));
    }
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn fixture(total: u64, idle: u64) -> Metrics {
        Metrics {
            cpu: Some(CpuTimes { total, idle }),
            memory: Some(Usage {
                total: 8 << 30,
                used: 3 << 30,
                free: 5 << 30,
            }),
            disk: Some(Usage {
                total: 64 << 30,
                used: 20 << 30,
                free: 40 << 30,
            }),
            uptime: Some(183840),
            load: Some([0.25, 0.5, 0.75]),
            ..Metrics::default()
        }
    }

    #[test]
    fn cpu_uses_deltas_and_rejects_missing_stopped_or_reset_counters() {
        let prev = CpuTimes {
            total: 100,
            idle: 50,
        };
        assert_eq!(cpu_busy(None, prev), None);
        assert_eq!(cpu_busy(Some(prev), prev), None);
        assert_eq!(
            cpu_busy(
                Some(prev),
                CpuTimes {
                    total: 200,
                    idle: 125
                }
            ),
            Some(0.25)
        );
        assert_eq!(
            cpu_busy(
                Some(prev),
                CpuTimes {
                    total: 99,
                    idle: 51
                }
            ),
            None
        );
        assert_eq!(
            cpu_busy(
                Some(prev),
                CpuTimes {
                    total: 200,
                    idle: 49
                }
            ),
            None
        );
        assert_eq!(
            cpu_busy(
                Some(prev),
                CpuTimes {
                    total: 200,
                    idle: 250
                }
            ),
            Some(0.0)
        );
    }

    #[test]
    fn readings_stay_with_their_session_and_stale_cpu_does_not_span_hidden_time() {
        let at = Instant::now();
        let mut monitor = Monitor::default();
        assert_eq!(monitor.record(1, fixture(100, 50), at), FIRST_CPU);
        assert_eq!(
            monitor.record(1, fixture(200, 125), at + INTERVAL),
            INTERVAL
        );
        assert_eq!(monitor.readings[&1].as_ref().unwrap().cpu, Some(0.25));
        monitor.record(2, fixture(1000, 10), at + INTERVAL);
        assert_eq!(monitor.readings[&2].as_ref().unwrap().cpu, None);
        assert_eq!(
            monitor.record(1, fixture(1000, 300), at + Duration::from_secs(13)),
            FIRST_CPU
        );
        let reading = monitor.readings[&1].as_ref().unwrap();
        assert_eq!(reading.history.back().unwrap().values[0], None);
        assert_eq!(reading.cpu, Some(0.25));
        monitor.record(1, fixture(1200, 400), at + Duration::from_secs(80));
        assert_eq!(monitor.readings[&1].as_ref().unwrap().history.len(), 1);
        monitor.retain(|id| id == 2);
        assert!(!monitor.readings.contains_key(&1));
    }

    #[test]
    fn empty_servers_back_off_but_partial_linux_readings_are_shown() {
        let mut monitor = Monitor::default();
        assert_eq!(
            monitor.record(1, Metrics::default(), Instant::now()),
            UNSUPPORTED
        );
        assert!(monitor.readings[&1].is_none());
        let mut partial = fixture(100, 0);
        partial.cpu = None;
        assert_eq!(monitor.record(1, partial, Instant::now()), INTERVAL);
        assert!(monitor.readings[&1].is_some());
    }

    #[test]
    fn polling_has_one_request_and_discards_late_results_after_switching_or_hiding() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(
            Core::with_secrets(directory.path(), nexssh_core::secrets::Secrets::in_memory())
                .unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut monitor = Monitor::default();
        let now = Instant::now();
        monitor.tick(&core, &runtime, Some(1), now);
        let task = monitor.task.as_ref().unwrap().id();
        let generation = monitor.generation;
        monitor.tick(&core, &runtime, Some(1), now + Duration::from_secs(30));
        assert_eq!(monitor.task.as_ref().unwrap().id(), task);
        monitor.tick(&core, &runtime, Some(2), now);
        assert_ne!(monitor.task.as_ref().unwrap().id(), task);
        monitor
            .sender
            .send(Completion {
                session: 1,
                generation,
                at: now,
                result: Ok(fixture(100, 0)),
            })
            .unwrap();
        monitor.tick(&core, &runtime, Some(2), now);
        assert!(!monitor.readings.contains_key(&1));
        let generation = monitor.generation;
        monitor.stop();
        monitor
            .sender
            .send(Completion {
                session: 2,
                generation,
                at: now,
                result: Ok(fixture(100, 0)),
            })
            .unwrap();
        monitor.tick(&core, &runtime, None, now);
        assert!(monitor.task.is_none());
        assert!(monitor.readings.is_empty());
    }

    #[test]
    fn error_backoff_keeps_the_last_good_reading_and_does_not_restart_early() {
        let directory = tempfile::tempdir().unwrap();
        let core = Arc::new(
            Core::with_secrets(directory.path(), nexssh_core::secrets::Secrets::in_memory())
                .unwrap(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut monitor = Monitor::default();
        let at = Instant::now();
        monitor.record(1, fixture(100, 0), at);
        monitor.tick(&core, &runtime, Some(1), at);
        monitor
            .sender
            .send(Completion {
                session: 1,
                generation: monitor.generation,
                at,
                result: Err("exec refused".into()),
            })
            .unwrap();
        monitor.tick(&core, &runtime, Some(1), at + INTERVAL);
        assert!(monitor.task.is_none());
        assert_eq!(monitor.next, at + RETRY);
        assert!(monitor.readings[&1].is_some());
    }

    #[test]
    fn values_follow_the_original_units_reserved_disk_blocks_and_uptime_rules() {
        assert_eq!(size(3 << 30, "ru"), "3,0 ГБ");
        assert_eq!(size(3 << 30, "en"), "3.0 GB");
        assert_eq!(size((1023.9 * 1024.0 * 1024.0) as u64, "en"), "1.0 GB");
        assert_eq!(
            pair(
                Usage {
                    total: 8 << 30,
                    used: 3 << 30,
                    free: 5 << 30
                },
                "ru"
            ),
            "3,0 / 8,0 ГБ"
        );
        assert_eq!(percent(0.125, "en"), "13%");
        assert_eq!(
            used_share(Usage {
                total: 120,
                used: 80,
                free: 20
            }),
            0.8
        );
        assert_eq!(uptime(183840, "ru"), "2 д 3 ч");
        assert_eq!(uptime(3600, "en"), "1 h");
        assert_eq!(uptime(59, "en"), "0 min");
    }

    #[test]
    fn graphs_show_lone_samples_and_break_at_missing_data_or_long_gaps() {
        let at = Instant::now();
        let history = VecDeque::from([
            Sample {
                at,
                values: [Some(0.25); 3],
            },
            Sample {
                at: at + INTERVAL,
                values: [None; 3],
            },
            Sample {
                at: at + INTERVAL * 2,
                values: [Some(0.5); 3],
            },
            Sample {
                at: at + Duration::from_secs(30),
                values: [Some(1.0); 3],
            },
        ]);
        let svg = graph(&history, 0);
        assert_eq!(svg.matches("stroke-width").count(), 3);
        let image = slint::Image::load_from_svg_data(svg.as_bytes()).unwrap();
        assert_eq!(image.size().width, 60);
        assert!(!svg.contains("NaN"));
    }

    #[test]
    fn ui_clears_other_sessions_and_reformats_cached_data_after_language_changes() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        i18n::init(&ui, "ru");
        let mut monitor = Monitor::default();
        let at = Instant::now();
        monitor.record(1, fixture(100, 50), at);
        monitor.record(1, fixture(200, 125), at + INTERVAL);
        monitor.apply(&ui, Some(1), "ru");
        assert_eq!(ui.get_status_metrics().row_count(), 3);
        assert_eq!(
            ui.get_status_metrics().row_data(0).unwrap().value,
            "25\u{a0}%"
        );
        assert_eq!(ui.get_status_uptime(), "работает 2 д 3 ч");
        monitor.apply(&ui, Some(1), "en");
        let row = ui.get_status_metrics().row_data(1).unwrap();
        assert_eq!(row.label, "RAM");
        assert_eq!(row.value, "3.0 / 8.0 GB");
        assert!(row.detail.contains("Last minute"));
        monitor.apply(&ui, None, "en");
        assert_eq!(ui.get_status_metrics().row_count(), 0);
        assert!(ui.get_status_uptime().is_empty());
        monitor.record(2, Metrics::default(), at);
        monitor.apply(&ui, Some(2), "en");
        assert!(ui.get_metrics_unavailable());
        assert_eq!(ui.get_status_metrics().row_count(), 0);
        monitor.apply(&ui, Some(1), "en");
        assert!(!ui.get_metrics_unavailable());
        assert_eq!(ui.get_status_metrics().row_count(), 3);
    }
}
