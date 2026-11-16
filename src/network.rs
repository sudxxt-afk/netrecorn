//! Async network layer.
//!
//! Engines:
//! * **caracat** — Paris-traceroute packet engine (ICMP/ICMPv6, raw sockets).
//!   It is fully blocking (libpcap), so the whole send/receive loop runs in
//!   `spawn_blocking` and streams [`NetEvent`]s through an mpsc channel.
//! * **system traceroute** — fallback when raw sockets are unavailable
//!   (no root / `CAP_NET_RAW`); stdout is parsed line-by-line as it arrives.
//!
//! Enrichment (GeoIP + reverse DNS) runs as separate tokio tasks with a
//! concurrency limit of 5 and a persistent on-disk cache, so the UI never
//! blocks and ip-api.com rate limits are respected.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc::UnboundedSender, Semaphore};
use tokio::task::JoinHandle;

use crate::models::{
    continent_of, is_private_ip, GeoInfo, NetEvent, RunEvent, MAX_HOPS, PROBES_PER_HOP,
};

/// Why an engine could not run.
#[derive(Debug)]
enum EngineError {
    /// Raw socket / capture permission problem.
    Permission(String),
    /// Anything else.
    Other(String),
}

/// Classify an engine error by inspecting its message (pcap/pnet/OS errors
/// are untyped strings, so this is the pragmatic approach).
fn classify_error(message: String) -> EngineError {
    let lower = message.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "permission",
        "not permitted",
        "access denied",
        "eperm",
        "requires root",
        "must be root",
        "don't have permission",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) {
        EngineError::Permission(message)
    } else {
        EngineError::Other(message)
    }
}

fn emit(tx: &UnboundedSender<RunEvent>, run_id: u64, event: NetEvent) {
    let _ = tx.send(RunEvent { run_id, event });
}

/// Spawn one traceroute run. The returned handle can be aborted; the engine
/// additionally polls `cancel` so blocking work stops promptly.
pub fn spawn_run(
    target: String,
    tx: UnboundedSender<RunEvent>,
    run_id: u64,
    cancel: Arc<AtomicBool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(err) = run_inner(&target, &tx, run_id, &cancel).await {
            emit(&tx, run_id, NetEvent::Error(format!("{err:#}")));
        }
    })
}

async fn run_inner(
    target: &str,
    tx: &UnboundedSender<RunEvent>,
    run_id: u64,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    // Fresh enrichment dedup set for this run.
    if let Some(seen) = enrich_seen() {
        seen.lock().unwrap().clear();
    }

    let target_ip = resolve(target).await?;
    emit(tx, run_id, NetEvent::Started { target_ip, engine: "caracat" });

    // Our own public IP (map origin) — independent of the traceroute.
    {
        let tx = tx.clone();
        tokio::spawn(async move {
            if let Some(origin) = geo::lookup_self().await {
                emit(&tx, run_id, NetEvent::Origin(Box::new(origin)));
            }
        });
    }

    // Primary engine: caracat in a dedicated blocking task.
    let engine_tx = tx.clone();
    let engine_cancel = Arc::clone(cancel);
    let result = tokio::task::spawn_blocking(move || {
        caracat_engine(target_ip, &engine_tx, run_id, &engine_cancel)
    })
    .await?;

    match result {
        Ok(()) => Ok(()),
        Err(engine_err) => {
            // Fallback: system traceroute (may be setuid or use ICMP sockets).
            let reason = match &engine_err {
                EngineError::Permission(m) => format!("raw sockets unavailable ({m})"),
                EngineError::Other(m) => m.clone(),
            };
            emit(
                tx,
                run_id,
                NetEvent::Notice(format!("caracat: {reason} — using system traceroute")),
            );
            emit(tx, run_id, NetEvent::Started { target_ip, engine: "system traceroute" });
            system_traceroute(target_ip, tx, run_id, cancel).await.map_err(|err| match err {
                EngineError::Permission(_) => anyhow!(
                    "Raw socket permission denied. Try running with sudo or \
                     setcap cap_net_raw,cap_net_admin+ep on the tracemap binary."
                ),
                EngineError::Other(m) => anyhow!("traceroute failed: {m}"),
            })
        }
    }
}

/// Resolve the target hostname to an IP (blocking getaddrinfo off-runtime).
async fn resolve(target: &str) -> Result<IpAddr> {
    // A bare IP parses directly without DNS.
    if let Ok(ip) = target.parse::<IpAddr>() {
        return Ok(ip);
    }
    let host = target.to_owned();
    let addrs = tokio::task::spawn_blocking(move || (host.as_str(), 0u16).to_socket_addrs())
        .await?
        .map_err(|e| anyhow!("DNS resolution failed for '{target}': {e}"))?;
    addrs
        .map(|sa| sa.ip())
        .next()
        .ok_or_else(|| anyhow!("DNS resolution failed for '{target}': no addresses returned"))
}

// ---------------------------------------------------------------------------
// caracat engine (blocking)
// ---------------------------------------------------------------------------

/// Paris-traceroute loop over caracat's `Sender`/`Receiver`.
///
/// Sends [`PROBES_PER_HOP`] ICMP probes per TTL with a constant flow ID,
/// drains matching ICMP replies until the per-hop deadline, and emits
/// cumulative [`NetEvent::Probe`] updates so the UI fills in live.
fn caracat_engine(
    target: IpAddr,
    tx: &UnboundedSender<RunEvent>,
    run_id: u64,
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    use caracat::models::{Probe, L4};
    use caracat::receiver::Receiver;
    use caracat::sender::Sender;
    use caracat::utilities::get_default_interface;

    let map_err = |e: anyhow::Error| classify_error(format!("{e:#}"));

    let iface = get_default_interface();
    if iface.is_empty() {
        return Err(EngineError::Other("no default network interface found".into()));
    }
    let instance_id: u16 = rand::random();
    let mut sender = Sender::new(&iface, None, None, instance_id, false).map_err(map_err)?;
    // Batch mode with a 100 ms packet-buffer timeout. RTT accuracy is
    // unaffected — packets are timestamped at capture time.
    let mut receiver = Receiver::new(&iface, 1024 * 1024, 100, false).map_err(map_err)?;
    // `next_reply()` blocks until a packet actually arrives: the pcap crate
    // swallows read timeouts internally, so neither interactive nor batch
    // mode bounds the wait. Move blocking reads into a worker thread and
    // poll a std channel with a timeout — the per-hop deadline below is
    // then always honoured. The worker exits once the engine drops the
    // channel (run finished / cancelled).
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        match receiver.next_reply() {
            Ok(reply) => {
                if reply_tx.send(reply).is_err() {
                    break; // engine gone (run finished or cancelled)
                }
            }
            // Parse failure on an exotic packet — not fatal, keep capturing.
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    });

    let protocol = match target {
        IpAddr::V4(_) => L4::ICMP,
        IpAddr::V6(_) => L4::ICMPv6,
    };
    let mut reached = false;

    for ttl in 1..=MAX_HOPS {
        if cancel.load(Ordering::Relaxed) || tx.is_closed() {
            return Ok(());
        }

        let mut recv = 0u8;
        let mut rtt_sum = 0.0f64;
        let mut hop_ip: Option<IpAddr> = None;

        for _ in 0..PROBES_PER_HOP {
            let probe = Probe {
                dst_addr: target,
                src_port: 24000, // constant Paris flow ID
                dst_port: 33434,
                ttl,
                protocol,
            };
            sender.send(&probe).map_err(map_err)?;
            std::thread::sleep(Duration::from_millis(60));
        }

        let deadline = Instant::now() + Duration::from_millis(2200);
        while recv < PROBES_PER_HOP && Instant::now() < deadline {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let reply = match reply_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(reply) => reply,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            // The receiver sees ALL ICMP on the interface — filter.
            if reply.probe_dst_addr != target || !reply.is_valid(instance_id) {
                continue;
            }
            // Echo replies (final hop) carry no quoted probe TTL.
            if reply.probe_ttl != ttl && reply.reply_src_addr != target {
                continue;
            }
            recv += 1;
            rtt_sum += f64::from(reply.rtt) / 10.0; // rtt is in 0.1 ms units
            hop_ip = Some(reply.reply_src_addr);
            emit(
                tx,
                run_id,
                NetEvent::Probe {
                    hop_num: ttl,
                    ip: hop_ip,
                    rtt_ms: Some(rtt_sum / f64::from(recv)),
                    sent: PROBES_PER_HOP,
                    recv,
                },
            );
            if let Some(ip) = hop_ip {
                enqueue_enrichment(ip, tx.clone(), run_id);
            }
        }

        emit(
            tx,
            run_id,
            NetEvent::Probe {
                hop_num: ttl,
                ip: hop_ip,
                rtt_ms: if recv > 0 { Some(rtt_sum / f64::from(recv)) } else { None },
                sent: PROBES_PER_HOP,
                recv,
            },
        );
        if hop_ip == Some(target) {
            reached = true;
            break;
        }
    }

    emit(tx, run_id, NetEvent::Finished { reached });
    Ok(())
}

// ---------------------------------------------------------------------------
// system traceroute fallback (async, streaming stdout)
// ---------------------------------------------------------------------------

async fn system_traceroute(
    target_ip: IpAddr,
    tx: &UnboundedSender<RunEvent>,
    run_id: u64,
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    let args = vec![
        "-n".to_owned(),
        "-q".to_owned(),
        PROBES_PER_HOP.to_string(),
        "-w".to_owned(),
        "2".to_owned(),
        "-m".to_owned(),
        MAX_HOPS.to_string(),
        target_ip.to_string(),
    ];
    let mut child = Command::new("traceroute")
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| EngineError::Other(format!("cannot spawn traceroute: {e}")))?;

    let stdout = child.stdout.take().expect("stdout piped");
    let mut lines = BufReader::new(stdout).lines();
    let mut reached = false;
    let mut hops_seen = 0usize;

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| EngineError::Other(format!("reading traceroute output: {e}")))?
    {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill().await;
            return Ok(());
        }
        let Some((hop_num, ip, rtts, timeouts)) = parse_traceroute_line(&line) else {
            continue;
        };
        hops_seen += 1;
        let sent = (rtts.len() + usize::from(timeouts)).min(255) as u8;
        let recv = rtts.len().min(255) as u8;
        let rtt_ms =
            if recv > 0 { Some(rtts.iter().sum::<f64>() / f64::from(recv)) } else { None };
        emit(tx, run_id, NetEvent::Probe { hop_num, ip, rtt_ms, sent, recv });
        if let Some(ip) = ip {
            enqueue_enrichment(ip, tx.clone(), run_id);
            if ip == target_ip {
                reached = true;
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| EngineError::Other(format!("waiting for traceroute: {e}")))?;
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr).await;
    }

    if hops_seen == 0 && (!status.success() || !stderr.trim().is_empty()) {
        return Err(classify_error(format!(
            "traceroute exited with {status}: {}",
            stderr.trim()
        )));
    }
    emit(tx, run_id, NetEvent::Finished { reached });
    Ok(())
}

/// Parse one line of `traceroute -n` output (Linux and macOS share the format).
///
/// Examples:
/// ```text
///  1  192.168.1.1  0.512 ms  0.480 ms  0.455 ms
///  2  * * *
///  3  10.0.0.1  1.200 ms *  1.300 ms
/// ```
/// Returns `(hop_num, ip, rtts_ms, timeout_probes)` or `None` for headers/junk.
pub(crate) fn parse_traceroute_line(line: &str) -> Option<(u8, Option<IpAddr>, Vec<f64>, u8)> {
    let mut tokens = line.split_whitespace();
    let hop_num: u8 = tokens.next()?.parse().ok()?;
    let mut ip = None;
    let mut rtts = Vec::new();
    let mut timeouts = 0u8;
    let rest: Vec<&str> = tokens.collect();
    let mut i = 0;
    while i < rest.len() {
        let token = rest[i];
        if token == "*" {
            timeouts = timeouts.saturating_add(1);
        } else if let Ok(rtt) = token.parse::<f64>() {
            rtts.push(rtt);
            if rest.get(i + 1) == Some(&"ms") {
                i += 1; // skip the unit
            }
        } else if let Ok(addr) = token.parse::<IpAddr>() {
            ip = Some(addr);
        }
        // Anything else ("ms", "!H", annotations…) is ignored.
        i += 1;
    }
    if ip.is_none() && rtts.is_empty() && timeouts == 0 {
        return None;
    }
    Some((hop_num, ip, rtts, timeouts))
}

// ---------------------------------------------------------------------------
// GeoIP + reverse DNS enrichment
// ---------------------------------------------------------------------------

/// At most 5 concurrent ip-api.com requests (free tier: 45 req/min).
static GEO_SEM: Semaphore = Semaphore::const_new(5);

fn enrich_seen() -> Option<&'static Mutex<HashSet<IpAddr>>> {
    static SEEN: OnceLock<Mutex<HashSet<IpAddr>>> = OnceLock::new();
    Some(SEEN.get_or_init(|| Mutex::new(HashSet::new())))
}

/// Kick off geo + reverse DNS lookups for a newly discovered public IP.
/// Deduplicated per run; the disk cache deduplicates across runs.
fn enqueue_enrichment(ip: IpAddr, tx: UnboundedSender<RunEvent>, run_id: u64) {
    if is_private_ip(ip) {
        return; // private hops get context from the UI, no API calls
    }
    let Some(seen) = enrich_seen() else { return };
    if !seen.lock().unwrap().insert(ip) {
        return;
    }
    tokio::spawn(async move {
        let geo_info = geo::lookup(ip).await;
        emit(&tx, run_id, NetEvent::Geo { ip, geo: Box::new(geo_info) });

        let rev = tokio::task::spawn_blocking(move || dns_lookup::lookup_addr(&ip)).await;
        if let Ok(Ok(hostname)) = rev {
            if hostname != ip.to_string() {
                emit(&tx, run_id, NetEvent::Hostname { ip, hostname });
            }
        }
    });
}

/// Persistent GeoIP cache + ip-api.com client.
pub mod geo {
    use super::*;

    /// Cache entries older than this are refreshed.
    const TTL_SECS: i64 = 30 * 24 * 3600;
    const API_FIELDS: &str = "status,country,countryCode,city,lat,lon,as,org,hosting,query";

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct CacheEntry {
        geo: GeoInfo,
        /// Unix timestamp of the lookup.
        at: i64,
    }

    pub struct GeoCache {
        entries: Mutex<HashMap<String, CacheEntry>>,
        path: PathBuf,
    }

    impl GeoCache {
        fn global() -> Arc<GeoCache> {
            static CACHE: OnceLock<Arc<GeoCache>> = OnceLock::new();
            Arc::clone(CACHE.get_or_init(|| Arc::new(Self::load())))
        }

        fn load() -> Self {
            let path = dirs::cache_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("tracemap")
                .join("geo.json");
            let entries = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<HashMap<String, CacheEntry>>(&raw).ok())
                .unwrap_or_default();
            Self { entries: Mutex::new(entries), path }
        }

        fn get(&self, key: &str) -> Option<GeoInfo> {
            let entries = self.entries.lock().unwrap();
            let entry = entries.get(key)?;
            if chrono::Utc::now().timestamp() - entry.at > TTL_SECS {
                return None;
            }
            Some(entry.geo.clone())
        }

        fn insert(&self, key: String, geo: GeoInfo) {
            {
                let mut entries = self.entries.lock().unwrap();
                entries.insert(key, CacheEntry { geo, at: chrono::Utc::now().timestamp() });
            }
            self.save();
        }

        /// Persist the cache to disk (small file, called on insert and exit).
        pub fn save(&self) {
            let snapshot = self.entries.lock().unwrap().clone();
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(&snapshot) {
                let _ = std::fs::write(&self.path, json);
            }
        }
    }

    /// Persist the cache (called on app shutdown).
    pub fn save_cache() {
        GeoCache::global().save();
    }

    fn http() -> &'static reqwest::Client {
        static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
        CLIENT.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(6))
                .user_agent(concat!("tracemap/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client")
        })
    }

    /// Raw ip-api.com response (only the fields we request).
    #[derive(Debug, Deserialize)]
    struct IpApiResp {
        status: String,
        #[serde(default)]
        country: Option<String>,
        #[serde(default, rename = "countryCode")]
        country_code: Option<String>,
        #[serde(default)]
        city: Option<String>,
        #[serde(default)]
        lat: Option<f64>,
        #[serde(default)]
        lon: Option<f64>,
        #[serde(default, rename = "as")]
        as_field: Option<String>,
        #[serde(default)]
        org: Option<String>,
        #[serde(default)]
        hosting: Option<bool>,
        #[serde(default)]
        query: Option<String>,
    }

    impl IpApiResp {
        fn into_geo(self, fallback_ip: IpAddr) -> GeoInfo {
            let ip = self
                .query
                .as_deref()
                .and_then(|q| q.parse::<IpAddr>().ok())
                .unwrap_or(fallback_ip);
            let asn = self
                .as_field
                .as_deref()
                .and_then(|s| s.split_whitespace().next())
                .filter(|s| s.starts_with("AS"))
                .map(str::to_owned);
            let continent = self.country_code.as_deref().and_then(continent_of);
            GeoInfo {
                ip: ip.to_string(),
                asn,
                org: self.org,
                country: self.country,
                country_code: self.country_code,
                city: self.city,
                lat: self.lat,
                lon: self.lon,
                hosting: self.hosting.unwrap_or(false),
                continent: continent.map(str::to_owned),
            }
        }
    }

    async fn fetch(url: &str, fallback_ip: IpAddr) -> Option<GeoInfo> {
        let resp = http().get(url).send().await.ok()?.error_for_status().ok()?;
        let parsed: IpApiResp = resp.json().await.ok()?;
        if parsed.status != "success" {
            return None;
        }
        Some(parsed.into_geo(fallback_ip))
    }

    /// Geo/ASN for one IP. Never fails hard: on any error returns an empty
    /// [`GeoInfo`] so the UI can render `N/A` instead of spinning forever.
    pub async fn lookup(ip: IpAddr) -> GeoInfo {
        let cache = GeoCache::global();
        let key = ip.to_string();
        if let Some(cached) = cache.get(&key) {
            return cached;
        }
        let Ok(_permit) = GEO_SEM.acquire().await else {
            return GeoInfo::empty(ip);
        };
        let url = format!("http://ip-api.com/json/{ip}?fields={API_FIELDS}");
        match fetch(&url, ip).await {
            Some(geo) => {
                cache.insert(key, geo.clone());
                geo
            }
            None => GeoInfo::empty(ip),
        }
    }

    /// Our own public IP + geo (ip-api returns the caller's info for `/json/`).
    pub async fn lookup_self() -> Option<GeoInfo> {
        let cache = GeoCache::global();
        if let Some(cached) = cache.get("self") {
            return Some(cached);
        }
        let _permit = GEO_SEM.acquire().await.ok()?;
        let url = format!("http://ip-api.com/json/?fields={API_FIELDS}");
        let geo = fetch(&url, "0.0.0.0".parse().unwrap()).await?;
        cache.insert("self".to_owned(), geo.clone());
        Some(geo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_normal_line() {
        let (hop, ip, rtts, to) =
            parse_traceroute_line(" 1  192.168.1.1  0.512 ms  0.480 ms  0.455 ms").unwrap();
        assert_eq!(hop, 1);
        assert_eq!(ip.unwrap().to_string(), "192.168.1.1");
        assert_eq!(rtts.len(), 3);
        assert_eq!(to, 0);
    }

    #[test]
    fn parses_timeout_line() {
        let (hop, ip, rtts, to) = parse_traceroute_line(" 2  * * *").unwrap();
        assert_eq!(hop, 2);
        assert!(ip.is_none());
        assert!(rtts.is_empty());
        assert_eq!(to, 3);
    }

    #[test]
    fn parses_mixed_line() {
        let (hop, ip, rtts, to) =
            parse_traceroute_line(" 3  10.0.0.1  1.200 ms *  1.300 ms").unwrap();
        assert_eq!(hop, 3);
        assert_eq!(ip.unwrap().to_string(), "10.0.0.1");
        assert_eq!(rtts.len(), 2);
        assert_eq!(to, 1);
    }

    #[test]
    fn parses_ipv6_and_annotations() {
        let (hop, ip, rtts, _) =
            parse_traceroute_line(" 4  2001:4860:4860::8888  12.1 ms  11.9 ms  12.0 ms").unwrap();
        assert_eq!(hop, 4);
        assert!(ip.unwrap().is_ipv6());
        assert_eq!(rtts.len(), 3);
        let (_, _, rtts, _) =
            parse_traceroute_line(" 5  10.1.1.1  5.0 ms !H  5.1 ms !H  5.2 ms").unwrap();
        assert_eq!(rtts.len(), 3);
    }

    #[test]
    fn ignores_header_lines() {
        assert!(parse_traceroute_line(
            "traceroute to google.com (142.250.185.78), 30 hops max, 60 byte packets"
        )
        .is_none());
        assert!(parse_traceroute_line("").is_none());
        assert!(parse_traceroute_line("   ").is_none());
    }

    #[test]
    fn permission_classification() {
        assert!(matches!(
            classify_error("You don't have permission to capture on that device".into()),
            EngineError::Permission(_)
        ));
        assert!(matches!(
            classify_error("socket: Operation not permitted (os error 1)".into()),
            EngineError::Permission(_)
        ));
        assert!(matches!(
            classify_error("interface not found".into()),
            EngineError::Other(_)
        ));
    }
}
