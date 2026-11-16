//! Data models shared between the network layer, the app state and the UI.
//!
//! Everything the UI needs to know about a hop lives in [`Hop`]; geo/ASN
//! enrichment arrives asynchronously and is attached later via [`NetEvent::Geo`].

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// Maximum TTL we are willing to probe.
pub const MAX_HOPS: u8 = 30;
/// Probes sent per hop (used to estimate packet loss).
pub const PROBES_PER_HOP: u8 = 3;

/// Demo-grade heuristic "high risk" country list (edit to taste).
pub const HIGH_RISK_COUNTRIES: &[&str] = &["KP", "IR", "SY", "RU", "BY", "CN"];

/// Organisation substrings that mark a hop as hosting / datacenter.
pub const HOSTING_ORGS: &[&str] = &[
    "amazon",
    "aws",
    "google",
    "digitalocean",
    "microsoft",
    "azure",
    "ovh",
    "hetzner",
    "linode",
    "vultr",
    "cloudflare",
    "akamai",
    "fastly",
    "oracle",
    "alibaba",
    "tencent",
    "huawei",
    "scaleway",
    "contabo",
    "leaseweb",
    "rackspace",
    "softlayer",
    "godaddy",
];

/// Geo/ASN enrichment for one IP address (from ip-api.com, cached on disk).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GeoInfo {
    pub ip: String,
    /// Autonomous system number, e.g. `AS15169`.
    pub asn: Option<String>,
    /// Organisation / ISP name.
    pub org: Option<String>,
    pub country: Option<String>,
    pub country_code: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// ip-api `hosting` flag: datacenter / cloud IP.
    pub hosting: bool,
    /// Continent code derived from `country_code` (EU, AS, NA, SA, AF, OC, AN).
    pub continent: Option<String>,
}

impl GeoInfo {
    /// Placeholder used when the GeoIP API failed: UI renders `N/A`.
    pub fn empty(ip: IpAddr) -> Self {
        Self {
            ip: ip.to_string(),
            ..Default::default()
        }
    }

    /// True when the lookup produced no usable data.
    pub fn is_empty(&self) -> bool {
        self.asn.is_none() && self.country.is_none() && self.city.is_none() && self.lat.is_none()
    }
}

/// RTT severity bucket used for color coding in the table and on the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RttClass {
    /// < 50 ms
    Fast,
    /// 50–150 ms
    Medium,
    /// > 150 ms or any packet loss
    Slow,
    /// No response.
    Unknown,
}

/// One row of the route: probe results plus (optional) async enrichment.
#[derive(Debug, Clone)]
pub struct Hop {
    pub hop_num: u8,
    /// Address of the replying router; `None` means `* * *` (timeout).
    pub ip: Option<IpAddr>,
    /// Reverse DNS name, when it differs from the IP.
    pub hostname: Option<String>,
    /// Geo/ASN data; `None` while the lookup is still in flight.
    pub geo: Option<GeoInfo>,
    /// Average RTT over received probes, milliseconds.
    pub rtt_ms: Option<f64>,
    pub probes_sent: u8,
    pub probes_recv: u8,
    /// When the hop's IP was first seen (drives the map ripple animation).
    pub discovered_at: Option<std::time::Instant>,
}

impl Hop {
    pub fn new(hop_num: u8) -> Self {
        Self {
            hop_num,
            ip: None,
            hostname: None,
            geo: None,
            rtt_ms: None,
            probes_sent: 0,
            probes_recv: 0,
            discovered_at: None,
        }
    }

    /// Packet loss in percent (0–100).
    pub fn loss_pct(&self) -> f64 {
        if self.probes_sent == 0 {
            return 100.0;
        }
        let lost = self.probes_sent.saturating_sub(self.probes_recv);
        f64::from(lost) / f64::from(self.probes_sent) * 100.0
    }

    pub fn is_timeout(&self) -> bool {
        self.ip.is_none()
    }

    pub fn is_private(&self) -> bool {
        self.ip.is_some_and(is_private_ip)
    }

    /// Geo lookup still pending (public IP, no answer yet).
    pub fn geo_pending(&self) -> bool {
        self.ip.is_some() && !self.is_private() && self.geo.is_none()
    }

    pub fn rtt_class(&self) -> RttClass {
        match self.rtt_ms {
            None => RttClass::Unknown,
            Some(rtt) if self.loss_pct() > 0.0 || rtt > 150.0 => RttClass::Slow,
            Some(rtt) if rtt >= 50.0 => RttClass::Medium,
            Some(_) => RttClass::Fast,
        }
    }

    pub fn coords(&self) -> Option<(f64, f64)> {
        let geo = self.geo.as_ref()?;
        Some((geo.lon?, geo.lat?))
    }

    pub fn continent(&self) -> Option<&str> {
        self.geo.as_ref()?.continent.as_deref()
    }

    /// Hosting / datacenter IP or a country from the heuristic risk list.
    pub fn suspicious(&self) -> bool {
        let Some(geo) = &self.geo else { return false };
        if geo.hosting {
            return true;
        }
        if let Some(cc) = &geo.country_code {
            if HIGH_RISK_COUNTRIES.contains(&cc.as_str()) {
                return true;
            }
        }
        if let Some(org) = &geo.org {
            let org = org.to_ascii_lowercase();
            if HOSTING_ORGS.iter().any(|key| org.contains(key)) {
                return true;
            }
        }
        false
    }
}

/// RFC1918 / loopback / link-local / ULA detection.
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified()
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || (first & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (first & 0xffc0) == 0xfe80 // link local fe80::/10
        }
    }
}

/// Flat JSON export row (stable, documented format).
#[derive(Debug, Serialize)]
pub struct ExportHop {
    pub hop: u8,
    pub ip: Option<String>,
    pub asn: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub loss_pct: f64,
}

impl From<&Hop> for ExportHop {
    fn from(h: &Hop) -> Self {
        let geo = h.geo.as_ref();
        Self {
            hop: h.hop_num,
            ip: h.ip.map(|ip| ip.to_string()),
            asn: geo.and_then(|g| g.asn.clone()),
            country: geo.and_then(|g| g.country.clone()),
            city: geo.and_then(|g| g.city.clone()),
            lat: geo.and_then(|g| g.lat),
            lon: geo.and_then(|g| g.lon),
            rtt_ms: h.rtt_ms.map(|r| (r * 100.0).round() / 100.0),
            loss_pct: (h.loss_pct() * 10.0).round() / 10.0,
        }
    }
}

/// Events streamed from the network layer into the UI.
#[derive(Debug)]
pub enum NetEvent {
    /// Target resolved, engine selected.
    Started {
        target_ip: IpAddr,
        engine: &'static str,
    },
    /// Cumulative probe results for a hop (sent after every reply).
    Probe {
        hop_num: u8,
        ip: Option<IpAddr>,
        rtt_ms: Option<f64>,
        sent: u8,
        recv: u8,
    },
    /// Reverse DNS result.
    Hostname { ip: IpAddr, hostname: String },
    /// Geo/ASN enrichment for every hop with this IP.
    Geo { ip: IpAddr, geo: Box<GeoInfo> },
    /// Our own public IP + geo (map origin marker).
    Origin(Box<GeoInfo>),
    /// Traceroute finished (destination reached or max TTL exhausted).
    Finished { reached: bool },
    /// Fatal error for the status bar.
    Error(String),
    /// Informational toast (engine fallback, etc.).
    Notice(String),
}

/// [`NetEvent`] tagged with the run generation so stale events from an
/// aborted run (e.g. after pressing `r`) are ignored by the app.
#[derive(Debug)]
pub struct RunEvent {
    pub run_id: u64,
    pub event: NetEvent,
}

/// Coarse continent lookup from an ISO 3166-1 alpha-2 country code.
pub fn continent_of(country_code: &str) -> Option<&'static str> {
    Some(match country_code.to_ascii_uppercase().as_str() {
        "AL" | "AD" | "AT" | "BY" | "BE" | "BA" | "BG" | "HR" | "CY" | "CZ" | "DK" | "EE"
        | "FO" | "FI" | "FR" | "DE" | "GI" | "GR" | "HU" | "IS" | "IE" | "IM" | "IT" | "XK"
        | "LV" | "LI" | "LT" | "LU" | "MT" | "MD" | "MC" | "ME" | "NL" | "MK" | "NO" | "PL"
        | "PT" | "RO" | "RU" | "SM" | "RS" | "SK" | "SI" | "ES" | "SE" | "CH" | "UA" | "GB"
        | "VA" => "EU",
        "AF" | "AM" | "AZ" | "BH" | "BD" | "BT" | "BN" | "KH" | "CN" | "GE" | "HK" | "IN"
        | "ID" | "IR" | "IQ" | "IL" | "JP" | "JO" | "KZ" | "KW" | "KG" | "LA" | "LB" | "MO"
        | "MY" | "MV" | "MN" | "MM" | "NP" | "KP" | "OM" | "PK" | "PS" | "PH" | "QA" | "SA"
        | "SG" | "KR" | "LK" | "SY" | "TW" | "TJ" | "TH" | "TL" | "TR" | "TM" | "AE" | "UZ"
        | "VN" | "YE" => "AS",
        "DZ" | "AO" | "BJ" | "BW" | "BF" | "BI" | "CV" | "CM" | "CF" | "TD" | "KM" | "CG"
        | "CD" | "CI" | "DJ" | "EG" | "GQ" | "ER" | "SZ" | "ET" | "GA" | "GM" | "GH" | "GN"
        | "GW" | "KE" | "LS" | "LR" | "LY" | "MG" | "MW" | "ML" | "MR" | "MU" | "YT" | "MA"
        | "MZ" | "NA" | "NE" | "NG" | "RE" | "RW" | "SH" | "ST" | "SN" | "SC" | "SL" | "SO"
        | "ZA" | "SS" | "SD" | "TZ" | "TG" | "TN" | "UG" | "EH" | "ZM" | "ZW" => "AF",
        "AG" | "AI" | "AW" | "BS" | "BB" | "BZ" | "BM" | "BQ" | "CA" | "KY" | "CR" | "CU"
        | "CW" | "DM" | "DO" | "SV" | "GL" | "GD" | "GP" | "GT" | "HT" | "HN" | "JM" | "MQ"
        | "MX" | "MS" | "NI" | "PA" | "PR" | "BL" | "KN" | "LC" | "MF" | "PM" | "VC" | "SX"
        | "TT" | "TC" | "US" | "VG" | "VI" => "NA",
        "AR" | "BO" | "BR" | "CL" | "CO" | "EC" | "FK" | "GF" | "GY" | "PY" | "PE" | "SR"
        | "UY" | "VE" => "SA",
        "AS" | "AU" | "CK" | "FJ" | "PF" | "GU" | "KI" | "MH" | "FM" | "NR" | "NC" | "NZ"
        | "NU" | "NF" | "MP" | "PW" | "PG" | "PN" | "WS" | "SB" | "TK" | "TO" | "TV" | "VU"
        | "WF" => "OC",
        "AQ" => "AN",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop_with_rtt(rtt: Option<f64>, sent: u8, recv: u8) -> Hop {
        Hop {
            rtt_ms: rtt,
            probes_sent: sent,
            probes_recv: recv,
            ..Hop::new(1)
        }
    }

    #[test]
    fn loss_computation() {
        assert_eq!(hop_with_rtt(Some(10.0), 3, 3).loss_pct(), 0.0);
        assert!((hop_with_rtt(Some(10.0), 3, 2).loss_pct() - 33.333).abs() < 0.01);
        assert_eq!(hop_with_rtt(None, 3, 0).loss_pct(), 100.0);
        assert_eq!(Hop::new(1).loss_pct(), 100.0);
    }

    #[test]
    fn rtt_classes() {
        assert_eq!(hop_with_rtt(Some(49.9), 3, 3).rtt_class(), RttClass::Fast);
        assert_eq!(hop_with_rtt(Some(50.0), 3, 3).rtt_class(), RttClass::Medium);
        assert_eq!(
            hop_with_rtt(Some(150.0), 3, 3).rtt_class(),
            RttClass::Medium
        );
        assert_eq!(hop_with_rtt(Some(150.1), 3, 3).rtt_class(), RttClass::Slow);
        // Any loss makes the hop "slow" regardless of RTT.
        assert_eq!(hop_with_rtt(Some(10.0), 3, 2).rtt_class(), RttClass::Slow);
        assert_eq!(hop_with_rtt(None, 3, 0).rtt_class(), RttClass::Unknown);
    }

    #[test]
    fn private_detection() {
        assert!(is_private_ip("192.168.1.1".parse().unwrap()));
        assert!(is_private_ip("10.0.0.1".parse().unwrap()));
        assert!(is_private_ip("127.0.0.1".parse().unwrap()));
        assert!(is_private_ip("169.254.1.1".parse().unwrap()));
        assert!(is_private_ip("fc00::1".parse().unwrap()));
        assert!(is_private_ip("fe80::1".parse().unwrap()));
        assert!(!is_private_ip("8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip("2001:4860:4860::8888".parse().unwrap()));
    }

    #[test]
    fn continent_mapping() {
        assert_eq!(continent_of("US"), Some("NA"));
        assert_eq!(continent_of("us"), Some("NA"));
        assert_eq!(continent_of("DE"), Some("EU"));
        assert_eq!(continent_of("JP"), Some("AS"));
        assert_eq!(continent_of("BR"), Some("SA"));
        assert_eq!(continent_of("AU"), Some("OC"));
        assert_eq!(continent_of("ZZ"), None);
    }

    #[test]
    fn suspicious_flags() {
        let mut h = Hop::new(1);
        h.geo = Some(GeoInfo {
            hosting: true,
            ..Default::default()
        });
        assert!(h.suspicious());
        h.geo = Some(GeoInfo {
            org: Some("Amazon.com, Inc.".into()),
            ..Default::default()
        });
        assert!(h.suspicious());
        h.geo = Some(GeoInfo {
            country_code: Some("KP".into()),
            ..Default::default()
        });
        assert!(h.suspicious());
        h.geo = Some(GeoInfo {
            country_code: Some("DE".into()),
            ..Default::default()
        });
        assert!(!h.suspicious());
    }

    #[test]
    fn export_shape() {
        let mut h = hop_with_rtt(Some(12.345), 3, 3);
        h.ip = Some("8.8.8.8".parse().unwrap());
        h.geo = Some(GeoInfo {
            asn: Some("AS15169".into()),
            country: Some("United States".into()),
            city: Some("Mountain View".into()),
            lat: Some(37.4),
            lon: Some(-122.1),
            ..Default::default()
        });
        let json = serde_json::to_value(ExportHop::from(&h)).unwrap();
        assert_eq!(json["hop"], 1);
        assert_eq!(json["ip"], "8.8.8.8");
        assert_eq!(json["asn"], "AS15169");
        assert_eq!(json["rtt_ms"], 12.35);
        assert_eq!(json["loss_pct"], 0.0);
        for key in [
            "hop", "ip", "asn", "country", "city", "lat", "lon", "rtt_ms", "loss_pct",
        ] {
            assert!(json.get(key).is_some(), "missing export key {key}");
        }
    }
}
