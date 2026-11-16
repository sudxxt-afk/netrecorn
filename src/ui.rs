//! All rendering: status header, hop table, world-map canvas, keybinding footer.
//!
//! Cyber-recon palette: near-black background, neon cyan accents, RTT coded
//! green / yellow / red. The map uses Braille markers at high resolution so
//! continents stay dense and detailed.

use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CanvasLine, Map, MapResolution, Points};
use ratatui::widgets::{Block, BorderType, Cell, Gauge, Paragraph, Row, Table};
use ratatui::Frame;

use crate::app::{App, Status};
use crate::models::{Hop, RttClass, MAX_HOPS};

// -- palette -----------------------------------------------------------------

pub const BG: Color = Color::Rgb(16, 16, 24);
pub const PANEL: Color = Color::Rgb(20, 20, 30);
pub const FG: Color = Color::Rgb(230, 230, 240);
pub const ACCENT: Color = Color::Rgb(0, 255, 255);
pub const DIM: Color = Color::DarkGray;
pub const GOOD: Color = Color::Green;
pub const WARN: Color = Color::Yellow;
pub const BAD: Color = Color::Red;
pub const ORIGIN: Color = Color::Magenta;
const ROUTE_LINE: Color = Color::Rgb(90, 90, 130);

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn rtt_color(hop: &Hop) -> Color {
    match hop.rtt_class() {
        RttClass::Fast => GOOD,
        RttClass::Medium => WARN,
        RttClass::Slow => BAD,
        RttClass::Unknown => DIM,
    }
}

/// RGB channels of a hop's RTT color (for ripple fading).
fn rtt_rgb(hop: &Hop) -> (u8, u8, u8) {
    match hop.rtt_class() {
        RttClass::Fast => (0, 255, 0),
        RttClass::Medium => (255, 255, 0),
        RttClass::Slow => (255, 0, 0),
        RttClass::Unknown => (128, 128, 128),
    }
}

/// Linear blend of an RGB color towards the background (fake alpha fade).
fn fade_rgb((r, g, b): (u8, u8, u8), k: f64) -> Color {
    let lerp = |c: u8, bg: u8| (f64::from(c) + (f64::from(bg) - f64::from(c)) * k) as u8;
    Color::Rgb(lerp(r, 16), lerp(g, 16), lerp(b, 24))
}

/// Continent of a hop, falling back to the origin's continent for private
/// (LAN) hops so continent jumps survive silent/private gaps in the route.
fn effective_continent<'a>(app: &'a App, hop: &'a Hop) -> Option<&'a str> {
    hop.continent().or_else(|| {
        if hop.is_private() {
            app.origin.as_ref()?.continent.as_deref()
        } else {
            None
        }
    })
}

/// Continent jump relative to the last hop with a known continent.
fn crosses_continent(app: &App, idx: usize, hop: &Hop) -> bool {
    let current = effective_continent(app, hop);
    let previous = app.hops[..idx]
        .iter()
        .rev()
        .find_map(|h| effective_continent(app, h));
    match (previous, current) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    }
}

/// Truncate a string to `max` characters, appending an ellipsis.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

// -- root layout ---------------------------------------------------------------

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::new().bg(BG)), area);

    let [header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    // Give the world map as much room as possible: 70% on wide terminals
    // (table goes slim: #, IP, RTT, Loss + selected-hop detail line),
    // 60% mid-size, 40% on narrow ones.
    let map_pct = if area.width >= 120 {
        70
    } else if area.width >= 90 {
        60
    } else {
        40
    };
    let [left, map_area] = Layout::horizontal([
        Constraint::Percentage(100 - map_pct),
        Constraint::Percentage(map_pct),
    ])
    .areas(main);
    let [table_area, profile_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(left);

    render_header(frame, app, header);
    render_table(frame, app, table_area);
    render_profile(frame, app, profile_area);
    render_map(frame, app, map_area);
    render_footer(frame, app, footer);
}

// -- header --------------------------------------------------------------------

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let [left, center, right] = Layout::horizontal([
        Constraint::Percentage(42),
        Constraint::Percentage(33),
        Constraint::Percentage(25),
    ])
    .areas(area);
    let panel = Style::new().bg(PANEL);

    let mut title = vec![
        Span::styled(
            " tracemap",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" → ", Style::new().fg(DIM)),
        Span::styled(
            app.target.as_str(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(ip) = app.target_ip {
        title.push(Span::styled(format!(" ({ip})"), Style::new().fg(DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(title)).style(panel), left);

    let status_line = match app.status {
        Status::Running => {
            let spinner = SPINNER[(app.tick_count / 2) as usize % SPINNER.len()];
            let gauge = Gauge::default()
                .ratio((app.hop_count() as f64 / f64::from(MAX_HOPS)).min(1.0))
                .label(Span::styled(
                    format!("{spinner} tracing…"),
                    Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
                ))
                .gauge_style(Style::new().fg(ACCENT).bg(PANEL))
                .style(Style::new().bg(PANEL));
            frame.render_widget(gauge, center);
            None
        }
        Status::Done if app.reached => Some(Line::from(Span::styled(
            "✔ route complete",
            Style::new().fg(GOOD),
        ))),
        Status::Done => Some(Line::from(Span::styled(
            "⚠ target unreachable",
            Style::new().fg(WARN),
        ))),
        Status::Error => Some(Line::from(Span::styled(
            "✖ error — see Route panel",
            Style::new().fg(BAD).add_modifier(Modifier::BOLD),
        ))),
    };
    if let Some(line) = status_line {
        frame.render_widget(
            Paragraph::new(line)
                .style(panel)
                .alignment(ratatui::layout::Alignment::Center),
            center,
        );
    }

    let elapsed = app.elapsed();
    let stats = format!(
        "Hops: {}/{}  ◷ {:02}:{:02}",
        app.hop_count(),
        MAX_HOPS,
        elapsed.as_secs() / 60,
        elapsed.as_secs() % 60
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(stats, Style::new().fg(FG))))
            .style(panel)
            .alignment(ratatui::layout::Alignment::Right),
        right,
    );
}

// -- hop table -----------------------------------------------------------------

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    // Column tiers: slim (map-max layouts), tiny, compact, full.
    let slim = area.width < 55;
    let tiny = area.width < 68;
    let compact = area.width < 80;

    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Span::styled(
            " Route ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(BG));
    if let Some(error) = &app.error {
        block = block.title_bottom(Span::styled(
            format!(" ✖ {error} "),
            Style::new().fg(BAD).add_modifier(Modifier::BOLD),
        ));
    } else if slim {
        // Slim tables drop ASN/Location columns; keep their context for the
        // selected hop in a detail line under the table.
        if let Some(hop) = app.table_state.selected().and_then(|i| app.hops.get(i)) {
            let geo = hop.geo.as_ref();
            let asn = geo.and_then(|g| g.asn.as_deref()).unwrap_or("—");
            let loc = if hop.is_private() {
                "private network".to_owned()
            } else {
                match geo {
                    Some(g) if !g.is_empty() => format!(
                        "{}, {}",
                        g.country_code.as_deref().unwrap_or("??"),
                        g.city.as_deref().unwrap_or("—")
                    ),
                    _ => "N/A".to_owned(),
                }
            };
            let detail = format!(" ▸ {}: {asn} · {loc} ", hop.hop_num);
            block = block.title_bottom(Span::styled(
                truncate(&detail, usize::from(area.width).saturating_sub(2)),
                Style::new().fg(ACCENT),
            ));
        }
    }

    let header = if slim {
        Row::new(["#", "IP / Host", "RTT", "Loss"])
    } else if tiny {
        Row::new(["#", "IP / Host", "ASN", "Location", "RTT", "Loss"])
    } else {
        Row::new(["Hop", "IP / Host", "ASN", "Location", "RTT ms", "Loss %"])
    }
    .style(
        Style::new()
            .fg(ACCENT)
            .add_modifier(Modifier::BOLD)
            .bg(PANEL),
    );

    // Narrower tiers shrink fixed columns so IP / Host keeps usable width.
    let widths: Vec<Constraint> = if slim {
        vec![
            Constraint::Length(2), // #
            Constraint::Fill(1),   // IP / Host
            Constraint::Length(6), // RTT
            Constraint::Length(5), // Loss
        ]
    } else if tiny {
        vec![
            Constraint::Length(2),  // Hop
            Constraint::Fill(1),    // IP / Host
            Constraint::Length(8),  // ASN
            Constraint::Length(12), // Location
            Constraint::Length(6),  // RTT ms
            Constraint::Length(5),  // Loss %
        ]
    } else if compact {
        vec![
            Constraint::Length(3),  // Hop
            Constraint::Fill(1),    // IP / Host
            Constraint::Length(9),  // ASN
            Constraint::Length(16), // Location
            Constraint::Length(7),  // RTT ms
            Constraint::Length(6),  // Loss %
        ]
    } else {
        vec![
            Constraint::Length(4),  // Hop
            Constraint::Fill(1),    // IP / Host
            Constraint::Length(10), // ASN
            Constraint::Length(19), // Location
            Constraint::Length(8),  // RTT ms
            Constraint::Length(7),  // Loss %
        ]
    };
    let fixed: usize = if slim {
        13 + 7 // 3 gaps + highlight symbol + borders
    } else if tiny {
        33 + 9
    } else if compact {
        41 + 9
    } else {
        48 + 9
    };
    let loc_width = if tiny {
        12
    } else if compact {
        16
    } else {
        19
    };
    let host_width = usize::from(area.width).saturating_sub(fixed).max(12);
    let rows: Vec<Row> = app
        .hops
        .iter()
        .enumerate()
        .map(|(idx, hop)| hop_row(app, idx, hop, host_width, loc_width, slim))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .column_spacing(1)
        .row_highlight_style(
            Style::new()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    frame.render_stateful_widget(table, area, &mut app.table_state);

    // Animated empty state while the first probes are in flight.
    if app.hops.is_empty() && app.status == Status::Running {
        let inner = area.inner(ratatui::layout::Margin {
            horizontal: 2,
            vertical: 2,
        });
        if inner.height > 0 {
            let spinner = SPINNER[(app.tick_count / 2) as usize % SPINNER.len()];
            let line = Line::from(Span::styled(
                format!("{spinner} establishing uplink…"),
                Style::new().fg(DIM).add_modifier(Modifier::ITALIC),
            ));
            let row = Rect {
                x: inner.x,
                y: inner.y + inner.height / 2,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(
                Paragraph::new(line).alignment(ratatui::layout::Alignment::Center),
                row,
            );
        }
    }
}

/// One-line latency profile: a colored bar per hop (▂▃▅▆▇█ by RTT).
fn render_profile(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![Span::styled(" RTT ", Style::new().fg(DIM))];
    for hop in &app.hops {
        let (ch, color) = match hop.rtt_ms {
            None => ('·', DIM),
            Some(r) => {
                let ch = match r as u32 {
                    0..=24 => '▂',
                    25..=49 => '▃',
                    50..=74 => '▄',
                    75..=99 => '▅',
                    100..=149 => '▆',
                    150..=249 => '▇',
                    _ => '█',
                };
                (ch, rtt_color(hop))
            }
        };
        spans.push(Span::styled(ch.to_string(), Style::new().fg(color)));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(PANEL)),
        area,
    );
}

fn hop_row(
    app: &App,
    idx: usize,
    hop: &Hop,
    host_width: usize,
    loc_width: usize,
    slim: bool,
) -> Row<'static> {
    let dim = Style::new().fg(DIM);

    // Silent hop: `* * *` and dashes, all DarkGray.
    if hop.is_timeout() {
        let cells = vec![
            Cell::from(hop.hop_num.to_string()).style(dim),
            Cell::from("* * *").style(dim.add_modifier(Modifier::ITALIC)),
            Cell::from("—").style(dim),
            Cell::from("—").style(dim),
            Cell::from("—").style(dim),
            Cell::from("100%").style(dim),
        ];
        return Row::new(slim_cells(cells, slim));
    }

    let crosses = crosses_continent(app, idx, hop);
    let suspicious = hop.suspicious();
    let private = hop.is_private();

    // Hop number: continent jump → cyan bold, suspicious → yellow bold.
    let hop_style = if crosses {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else if suspicious {
        Style::new().fg(WARN).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG)
    };

    // IP / Host with ✈ (continent change) and ⚠ (hosting / risk) flags.
    let mut ip_spans = Vec::new();
    if crosses {
        ip_spans.push(Span::styled(
            "✈ ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    }
    if suspicious {
        ip_spans.push(Span::styled(
            "⚠ ",
            Style::new().fg(WARN).add_modifier(Modifier::BOLD),
        ));
    }
    let display = hop
        .hostname
        .clone()
        .or_else(|| hop.ip.map(|ip| ip.to_string()))
        .unwrap_or_default();
    let display = truncate(&display, host_width.saturating_sub(2)); // room for ✈/⚠ flags
    let ip_style = if private {
        Style::new().fg(BAD)
    } else {
        Style::new().fg(FG)
    };
    ip_spans.push(Span::styled(display, ip_style));

    // ASN / Location: always contextual — pending, private, N/A or real data.
    let (asn_text, asn_style, loc_text, loc_style) = if private {
        (
            "—".to_owned(),
            dim,
            "private network".to_owned(),
            Style::new().fg(BAD),
        )
    } else if hop.geo_pending() {
        (
            "…".to_owned(),
            dim,
            "resolving…".to_owned(),
            dim.add_modifier(Modifier::ITALIC),
        )
    } else {
        match &hop.geo {
            Some(geo) if !geo.is_empty() => {
                let asn = geo.asn.clone().unwrap_or_else(|| "N/A".to_owned());
                let country = geo
                    .country_code
                    .clone()
                    .or_else(|| geo.country.clone())
                    .unwrap_or_else(|| "??".to_owned());
                let city = geo.city.clone().unwrap_or_else(|| "—".to_owned());
                (
                    asn,
                    Style::new().fg(Color::Rgb(187, 154, 247)), // Tokyo Night purple
                    truncate(&format!("{country}, {city}"), loc_width),
                    Style::new().fg(Color::Rgb(122, 162, 247)), // Tokyo Night blue
                )
            }
            _ => ("N/A".to_owned(), dim, "N/A".to_owned(), dim),
        }
    };

    let rtt_text = hop
        .rtt_ms
        .map_or_else(|| "—".to_owned(), |rtt| format!("{rtt:.1}"));
    let loss = hop.loss_pct();
    let loss_style = if loss > 0.0 {
        Style::new().fg(BAD)
    } else {
        Style::new().fg(GOOD)
    };

    Row::new(slim_cells(
        vec![
            Cell::from(hop.hop_num.to_string()).style(hop_style),
            Cell::from(Line::from(ip_spans)),
            Cell::from(asn_text).style(asn_style),
            Cell::from(loc_text).style(loc_style),
            Cell::from(rtt_text).style(Style::new().fg(rtt_color(hop))),
            Cell::from(format!("{loss:.0}%")).style(loss_style),
        ],
        slim,
    ))
}

/// Slim tier keeps only #, IP / Host, RTT, Loss (ASN/Location move to the
/// selected-hop detail line under the table).
fn slim_cells(cells: Vec<Cell<'static>>, slim: bool) -> Vec<Cell<'static>> {
    if slim {
        vec![
            cells[0].clone(),
            cells[1].clone(),
            cells[4].clone(),
            cells[5].clone(),
        ]
    } else {
        cells
    }
}

// -- world map -------------------------------------------------------------------

fn render_map(frame: &mut Frame, app: &App, area: Rect) {
    // Inactive panel: dim border (the navigable Route table owns the cyan).
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::styled(
            " Route Map ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(BG));
    if area.width < 12 || area.height < 8 {
        frame.render_widget(block, area);
        return;
    }

    let show_labels = area.width > 45;
    let canvas = Canvas::default()
        .block(block)
        .marker(Marker::Braille)
        .x_bounds([-180.0, 180.0])
        .y_bounds([-90.0, 90.0])
        .paint(|ctx| {
            // Dim continents so the route pops.
            ctx.draw(&Map {
                resolution: MapResolution::High,
                color: DIM,
            });
            ctx.layer();
            let mut placed: Vec<(f64, f64)> = Vec::new();

            // Faint radar graticule every 30° — depth without noise.
            let grid: Vec<(f64, f64)> = (-150..=150)
                .step_by(30)
                .flat_map(|lon| {
                    (-60..=60)
                        .step_by(30)
                        .map(move |lat| (lon as f64, lat as f64))
                })
                .collect();
            ctx.draw(&Points {
                coords: &grid,
                color: Color::Rgb(32, 32, 46),
            });

            // Route polyline in hop order (skip hops without coordinates),
            // with a "packet" dot marching along each segment.
            let coords: Vec<(f64, f64)> = app.hops.iter().filter_map(Hop::coords).collect();
            for (i, window) in coords.windows(2).enumerate() {
                let color = if i + 2 == coords.len() {
                    ACCENT
                } else {
                    ROUTE_LINE
                };
                ctx.draw(&CanvasLine {
                    x1: window[0].0,
                    y1: window[0].1,
                    x2: window[1].0,
                    y2: window[1].1,
                    color,
                });
                let t = (app.tick_count as f64 * 0.02 + i as f64 * 0.37) % 1.0;
                ctx.draw(&Circle {
                    x: window[0].0 + (window[1].0 - window[0].0) * t,
                    y: window[0].1 + (window[1].1 - window[0].1) * t,
                    radius: 0.6,
                    color: ACCENT,
                });
            }

            // Hop markers: breathing dots, plus an expanding ripple that
            // fades out over ~2.5 s after the hop was discovered.
            for (i, hop) in app.hops.iter().enumerate() {
                let Some((x, y)) = hop.coords() else { continue };
                if let Some(at) = hop.discovered_at {
                    let age = at.elapsed().as_secs_f64();
                    if age < 2.5 {
                        let k = age / 2.5;
                        ctx.draw(&Circle {
                            x,
                            y,
                            radius: 1.0 + k * 4.0,
                            color: fade_rgb(rtt_rgb(hop), k),
                        });
                    }
                }
                let phase = app.tick_count as f64 * 0.18 + i as f64 * 0.9;
                // Soft halo behind each hop dot.
                ctx.draw(&Circle {
                    x,
                    y,
                    radius: 1.7,
                    color: fade_rgb(rtt_rgb(hop), 0.78),
                });
                ctx.draw(&Circle {
                    x,
                    y,
                    radius: 0.7 + 0.25 * phase.sin(),
                    color: rtt_color(hop),
                });
            }

            // Origin: our own public IP, drawn as a map-style pin
            // (head circle + V tail pointing at the exact spot), no text.
            if let Some(origin) = &app.origin {
                if let (Some(lat), Some(lon)) = (origin.lat, origin.lon) {
                    ctx.draw(&Circle {
                        x: lon,
                        y: lat + 2.6,
                        radius: 2.6,
                        color: fade_rgb((255, 0, 255), 0.78),
                    });
                    ctx.draw(&CanvasLine {
                        x1: lon - 1.2,
                        y1: lat + 1.8,
                        x2: lon,
                        y2: lat,
                        color: ORIGIN,
                    });
                    ctx.draw(&CanvasLine {
                        x1: lon + 1.2,
                        y1: lat + 1.8,
                        x2: lon,
                        y2: lat,
                        color: ORIGIN,
                    });
                    ctx.draw(&Circle {
                        x: lon,
                        y: lat + 2.6,
                        radius: 1.4,
                        color: ORIGIN,
                    });
                    ctx.draw(&Points {
                        coords: &[(lon, lat + 2.6)],
                        color: BG,
                    });
                }
            }

            // Frontier pulse while running; blinking white destination when done.
            if let Some(last) = app.hops.iter().rev().find(|h| h.coords().is_some()) {
                let (x, y) = last.coords().unwrap();
                let is_destination = app.reached && last.ip == app.target_ip;
                if app.status == Status::Running {
                    let pulse = (app.tick_count as f64 * 0.25).sin().abs();
                    ctx.draw(&Circle {
                        x,
                        y,
                        radius: 1.0 + pulse * 2.0,
                        color: rtt_color(last),
                    });
                } else if is_destination {
                    let blink = (app.tick_count / 6) % 2 == 0;
                    ctx.draw(&Circle {
                        x,
                        y,
                        radius: 2.0,
                        color: if blink { Color::White } else { ACCENT },
                    });
                }
            }

            // Arrival ping: expanding white ring from the destination.
            if let Some(at) = app.finished_at {
                let age = at.elapsed().as_secs_f64();
                if age < 2.0 {
                    if let Some((x, y)) = app
                        .hops
                        .iter()
                        .rev()
                        .find(|h| h.ip == app.target_ip)
                        .and_then(Hop::coords)
                    {
                        ctx.draw(&Circle {
                            x,
                            y,
                            radius: 2.0 + age * 6.0,
                            color: fade_rgb((255, 255, 255), age / 2.0),
                        });
                    }
                }
            }

            // City labels for key hops: first, last and continent jumps.
            // Greedy collision avoidance so nearby cities don't overprint.
            if show_labels {
                let geo_hops: Vec<(usize, &Hop)> = app
                    .hops
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| h.coords().is_some())
                    .collect();
                for (idx, hop) in &geo_hops {
                    let key = *idx == geo_hops[0].0
                        || *idx == geo_hops[geo_hops.len() - 1].0
                        || crosses_continent(app, *idx, hop);
                    if !key {
                        continue;
                    }
                    if let (Some(city), Some((x, y))) = (
                        hop.geo.as_ref().and_then(|g| g.city.as_deref()),
                        hop.coords(),
                    ) {
                        // ~75° lon ≈ a 14-char label at typical canvas widths.
                        if placed
                            .iter()
                            .any(|(px, py)| (px - x).abs() < 75.0 && (py - y).abs() < 8.0)
                        {
                            continue;
                        }
                        placed.push((x, y));
                        let text = truncate(city, 14);
                        // Keep labels inside the canvas: flip to the left of
                        // the point when they would overflow the right edge.
                        let width_deg = text.chars().count() as f64 * 5.0;
                        let lx = if x + 3.0 + width_deg > 178.0 {
                            x - 3.0 - width_deg
                        } else {
                            x + 3.0
                        };
                        let ly = (y - 3.0).clamp(-85.0, 85.0);
                        ctx.print(lx, ly, Span::styled(text, Style::new().fg(FG)));
                    }
                }
            }
        });
    frame.render_widget(canvas, area);
}

// -- footer ------------------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let key = Style::new().fg(ACCENT);
    let label = Style::new().fg(DIM);
    let mut spans = vec![
        Span::styled(" [↑/↓]", key),
        Span::styled(" Navigate  ", label),
        Span::styled("[e]", key),
        Span::styled(" Export JSON  ", label),
        Span::styled("[r]", key),
        Span::styled(" Re-run  ", label),
        Span::styled("[q]", key),
        Span::styled(" Quit", label),
        Span::styled(format!("   engine: {}", app.engine), label),
    ];
    if let Some((message, at)) = &app.notice {
        if at.elapsed() < Duration::from_secs(5) {
            spans.push(Span::styled(
                format!("   ✔ {message}"),
                Style::new().fg(GOOD),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(PANEL)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::GeoInfo;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn seeded_app() -> App {
        let mut app = App::new("google.com".into());
        app.target_ip = Some("142.250.185.78".parse().unwrap());
        app.status = Status::Done;
        app.reached = true;

        let mut h1 = Hop::new(1);
        h1.ip = Some("192.168.1.1".parse().unwrap());
        h1.rtt_ms = Some(1.2);
        h1.probes_sent = 3;
        h1.probes_recv = 3;

        let mut h2 = Hop::new(2);
        h2.ip = Some("8.8.8.8".parse().unwrap());
        h2.rtt_ms = Some(24.5);
        h2.probes_sent = 3;
        h2.probes_recv = 3;
        h2.geo = Some(GeoInfo {
            ip: "8.8.8.8".into(),
            asn: Some("AS15169".into()),
            org: Some("Google LLC".into()),
            country: Some("United States".into()),
            country_code: Some("US".into()),
            city: Some("Mountain View".into()),
            lat: Some(37.4),
            lon: Some(-122.1),
            hosting: true,
            continent: Some("NA".into()),
        });

        let mut h3 = Hop::new(3); // silent hop
        h3.probes_sent = 3;
        h3.probes_recv = 0;

        app.hops = vec![h1, h2, h3];
        app.table_state.select(Some(1));
        app.origin = Some(GeoInfo {
            ip: "1.2.3.4".into(),
            lat: Some(55.75),
            lon: Some(37.62),
            city: Some("Moscow".into()),
            country_code: Some("RU".into()),
            continent: Some("EU".into()),
            ..Default::default()
        });
        app
    }

    #[test]
    fn renders_full_layout() {
        // 280 cols → full table columns (full location strings).
        let mut terminal = Terminal::new(TestBackend::new(280, 40)).unwrap();
        let mut app = seeded_app();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("tracemap"), "header title");
        assert!(text.contains("google.com"), "target in header");
        assert!(text.contains("Route"), "table title");
        assert!(text.contains("Route Map"), "map title");
        assert!(text.contains("AS15169"), "ASN column");
        assert!(text.contains("US, Mountain View"), "location column");
        assert!(text.contains("* * *"), "silent hop");
        assert!(text.contains("private network"), "private hop context");
        assert!(text.contains("Navigate"), "footer keybindings");
        assert!(text.contains("Hops: 3/30"), "hop counter");
    }

    #[test]
    fn slim_layout_keeps_selected_hop_context() {
        // 140 cols → 70% map, slim table; ASN/city of the selected hop must
        // survive in the detail line under the table.
        let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
        let mut app = seeded_app();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("AS15169"), "selected hop ASN in detail line");
        assert!(text.contains("US, Mountain View"), "selected hop location");
        assert!(text.contains("Route Map"), "map present");
    }

    #[test]
    fn renders_error_state() {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut app = App::new("bad.host".into());
        app.status = Status::Error;
        app.error = Some("Raw socket permission denied. Try running with sudo or setcap.".into());
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("error"), "error indicator");
        assert!(
            text.contains("Raw socket permission denied"),
            "error detail"
        );
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        let mut app = seeded_app();
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
    }

    #[test]
    fn truncate_works() {
        assert_eq!(truncate("US, Mountain View", 19), "US, Mountain View");
        assert_eq!(truncate("Very Long City Name Here", 10), "Very Long…");
    }
}
