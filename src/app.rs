//! Application state: hop table, selection, run lifecycle, export.
//!
//! The app owns one [`mpsc`] receiver; every traceroute run gets a fresh
//! channel and a monotonically increasing `run_id` so events from an aborted
//! run can never pollute the current one.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::widgets::TableState;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::models::{ExportHop, GeoInfo, Hop, NetEvent, RunEvent, MAX_HOPS};
use crate::network;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Running,
    Done,
    Error,
}

pub struct App {
    pub target: String,
    pub target_ip: Option<IpAddr>,
    pub engine: &'static str,
    pub hops: Vec<Hop>,
    pub table_state: TableState,
    pub status: Status,
    pub reached: bool,
    pub error: Option<String>,
    /// Toast message + when it was set (footer, auto-hides after 5 s).
    pub notice: Option<(String, Instant)>,
    /// Our own public IP geo — the "you are here" map marker.
    pub origin: Option<GeoInfo>,
    pub started_at: Instant,
    /// When the route completed (drives the arrival ping animation).
    pub finished_at: Option<Instant>,
    pub tick_count: u64,
    pub should_quit: bool,
    pub(crate) rx: UnboundedReceiver<RunEvent>,

    frozen_elapsed: Option<Duration>,
    user_moved: bool,
    run_id: u64,
    cancel: Arc<AtomicBool>,
    tx: UnboundedSender<RunEvent>,
    run_handle: Option<JoinHandle<()>>,
}

impl App {
    pub fn new(target: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            target,
            target_ip: None,
            engine: "caracat",
            hops: Vec::new(),
            table_state: TableState::default(),
            status: Status::Running,
            reached: false,
            error: None,
            notice: None,
            origin: None,
            started_at: Instant::now(),
            finished_at: None,
            tick_count: 0,
            should_quit: false,
            rx,
            frozen_elapsed: None,
            user_moved: false,
            run_id: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            run_handle: None,
        }
    }

    /// (Re)start the traceroute, clearing all per-run state.
    pub fn start_run(&mut self) {
        if let Some(handle) = self.run_handle.take() {
            handle.abort();
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.run_id += 1;

        self.hops.clear();
        self.table_state.select(None);
        self.user_moved = false;
        self.status = Status::Running;
        self.reached = false;
        self.error = None;
        self.target_ip = None;
        self.started_at = Instant::now();
        self.finished_at = None;
        self.frozen_elapsed = None;

        let (tx, rx) = mpsc::unbounded_channel();
        self.tx = tx;
        self.rx = rx;
        self.run_handle = Some(network::spawn_run(
            self.target.clone(),
            self.tx.clone(),
            self.run_id,
            Arc::clone(&self.cancel),
        ));
    }

    /// Wall-clock time of the run (frozen once it finishes).
    pub fn elapsed(&self) -> Duration {
        self.frozen_elapsed
            .unwrap_or_else(|| self.started_at.elapsed())
    }

    /// Animation clock, advanced by the UI ticker (~80 ms).
    pub fn tick(&mut self) {
        self.tick_count = self.tick_count.wrapping_add(1);
    }

    pub fn handle_net(&mut self, ev: RunEvent) {
        if ev.run_id != self.run_id {
            return; // stale event from an aborted run
        }
        match ev.event {
            NetEvent::Started { target_ip, engine } => {
                self.target_ip = Some(target_ip);
                self.engine = engine;
            }
            NetEvent::Probe {
                hop_num,
                ip,
                rtt_ms,
                sent,
                recv,
            } => {
                if let Some(hop) = self.hops.iter_mut().find(|h| h.hop_num == hop_num) {
                    if hop.ip.is_none() && ip.is_some() {
                        hop.discovered_at = Some(Instant::now());
                    }
                    if ip.is_some() {
                        hop.ip = ip;
                    }
                    hop.rtt_ms = rtt_ms;
                    hop.probes_sent = sent;
                    hop.probes_recv = recv;
                } else {
                    let mut hop = Hop::new(hop_num);
                    hop.ip = ip;
                    hop.discovered_at = ip.map(|_| Instant::now());
                    hop.rtt_ms = rtt_ms;
                    hop.probes_sent = sent;
                    hop.probes_recv = recv;
                    let pos = self.hops.partition_point(|h| h.hop_num < hop_num);
                    self.hops.insert(pos, hop);
                }
                // Follow the frontier until the user takes over navigation.
                if !self.user_moved {
                    self.table_state
                        .select(Some(self.hops.len().saturating_sub(1)));
                }
            }
            NetEvent::Hostname { ip, hostname } => {
                for hop in &mut self.hops {
                    if hop.ip == Some(ip) {
                        hop.hostname = Some(hostname.clone());
                    }
                }
            }
            NetEvent::Geo { ip, geo } => {
                for hop in &mut self.hops {
                    if hop.ip == Some(ip) {
                        hop.geo = Some(*geo.clone());
                    }
                }
            }
            NetEvent::Origin(geo) => self.origin = Some(*geo),
            NetEvent::Finished { reached } => {
                if self.status != Status::Error {
                    self.status = Status::Done;
                    self.reached = reached;
                    if reached {
                        self.finished_at = Some(Instant::now());
                    }
                    self.frozen_elapsed = Some(self.started_at.elapsed());
                }
            }
            NetEvent::Error(message) => {
                self.status = Status::Error;
                self.error = Some(message);
                self.frozen_elapsed = Some(self.started_at.elapsed());
            }
            NetEvent::Notice(message) => self.set_notice(message),
        }
    }

    pub fn select_next(&mut self) {
        if self.hops.is_empty() {
            return;
        }
        self.user_moved = true;
        let idx = self.table_state.selected().unwrap_or(0);
        self.table_state
            .select(Some((idx + 1).min(self.hops.len() - 1)));
    }

    pub fn select_prev(&mut self) {
        if self.hops.is_empty() {
            return;
        }
        self.user_moved = true;
        let idx = self.table_state.selected().unwrap_or(0);
        self.table_state.select(Some(idx.saturating_sub(1)));
    }

    pub fn select_first(&mut self) {
        if !self.hops.is_empty() {
            self.user_moved = true;
            self.table_state.select(Some(0));
        }
    }

    pub fn select_last(&mut self) {
        if !self.hops.is_empty() {
            self.user_moved = true;
            self.table_state.select(Some(self.hops.len() - 1));
        }
    }

    pub fn set_notice(&mut self, message: String) {
        self.notice = Some((message, Instant::now()));
    }

    /// Serialize the current route to `tracemap_<host>_<timestamp>.json`.
    /// Returns the file name on success.
    pub fn export_json(&self) -> anyhow::Result<String> {
        let rows: Vec<ExportHop> = self.hops.iter().map(ExportHop::from).collect();
        let safe_host: String = self
            .target
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let filename = format!("tracemap_{safe_host}_{timestamp}.json");
        std::fs::write(&filename, serde_json::to_vec_pretty(&rows)?)?;
        Ok(filename)
    }

    /// Stop background work and persist the GeoIP cache.
    pub fn shutdown(&mut self) {
        self.should_quit = true;
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(handle) = self.run_handle.take() {
            handle.abort();
        }
        network::geo::save_cache();
    }

    /// Number of hops to show in the header (`Hops: n/30`).
    pub fn hop_count(&self) -> usize {
        self.hops.len().min(usize::from(MAX_HOPS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::GeoInfo;

    fn probe(run_id: u64, hop_num: u8, ip: Option<IpAddr>) -> RunEvent {
        RunEvent {
            run_id,
            event: NetEvent::Probe {
                hop_num,
                ip,
                rtt_ms: Some(10.0),
                sent: 3,
                recv: 3,
            },
        }
    }

    #[test]
    fn probe_upsert_and_order() {
        let mut app = App::new("example.com".into());
        app.run_id += 1; // bump generation without spawning real network work
        let run_id = app.run_id;
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        app.handle_net(probe(run_id, 2, Some(ip)));
        app.handle_net(probe(run_id, 1, Some("1.1.1.1".parse().unwrap())));
        assert_eq!(app.hops.len(), 2);
        assert_eq!(app.hops[0].hop_num, 1);
        assert_eq!(app.hops[1].hop_num, 2);
        // Repeated probe updates in place.
        app.handle_net(probe(run_id, 2, Some(ip)));
        assert_eq!(app.hops.len(), 2);
        // Selection follows the frontier.
        assert_eq!(app.table_state.selected(), Some(1));
    }

    #[test]
    fn stale_run_events_ignored() {
        let mut app = App::new("example.com".into());
        app.run_id += 1; // bump generation without spawning real network work
        let old = app.run_id;
        app.run_id += 1; // bump generation without spawning real network work // rerun bumps run_id
        app.handle_net(probe(old, 1, Some("1.1.1.1".parse().unwrap())));
        assert!(app.hops.is_empty());
    }

    #[test]
    fn geo_applies_to_all_hops_with_ip() {
        let mut app = App::new("example.com".into());
        app.run_id += 1; // bump generation without spawning real network work
        let run_id = app.run_id;
        let ip: IpAddr = "8.8.8.8".parse().unwrap();
        app.handle_net(probe(run_id, 1, Some(ip)));
        app.handle_net(probe(run_id, 2, Some(ip)));
        app.handle_net(RunEvent {
            run_id,
            event: NetEvent::Geo {
                ip,
                geo: Box::new(GeoInfo {
                    asn: Some("AS15169".into()),
                    ..GeoInfo::empty(ip)
                }),
            },
        });
        assert!(app
            .hops
            .iter()
            .all(|h| h.geo.as_ref().unwrap().asn.as_deref() == Some("AS15169")));
        assert!(!app.hops[0].geo_pending());
    }

    #[test]
    fn navigation_clamps() {
        let mut app = App::new("example.com".into());
        app.run_id += 1; // bump generation without spawning real network work
        let run_id = app.run_id;
        for n in 1..=3 {
            app.handle_net(probe(run_id, n, Some("1.1.1.1".parse().unwrap())));
        }
        app.select_prev();
        app.select_prev();
        app.select_prev();
        app.select_prev();
        assert_eq!(app.table_state.selected(), Some(0));
        for _ in 0..10 {
            app.select_next();
        }
        assert_eq!(app.table_state.selected(), Some(2));
    }

    #[test]
    fn error_and_finish_states() {
        let mut app = App::new("example.com".into());
        app.run_id += 1; // bump generation without spawning real network work
        let run_id = app.run_id;
        app.handle_net(RunEvent {
            run_id,
            event: NetEvent::Finished { reached: true },
        });
        assert_eq!(app.status, Status::Done);
        assert!(app.reached);
        app.handle_net(RunEvent {
            run_id,
            event: NetEvent::Error("boom".into()),
        });
        assert_eq!(app.status, Status::Error);
        assert_eq!(app.error.as_deref(), Some("boom"));
    }
}
