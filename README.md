<div align="center">

```
███╗   ██╗███████╗████████╗██████╗ ███████╗ ██████╗ ██████╗ ██████╗ ███╗   ██╗
████╗  ██║██╔════╝╚══██╔══╝██╔══██╗██╔════╝██╔═══██╗██╔══██╗████╗  ██║
██╔██╗ ██║█████╗     ██║   ██████╔╝█████╗  ██║   ██║██████╔╝██╔██╗ ██║
██║╚██╗██║██╔══╝     ██║   ██╔══██╗██╔══╝  ██║   ██║██╔══██╗██║╚██╗██║
██║ ╚████║███████╗   ██║   ██║  ██║███████╗╚██████╔╝██║  ██║██║ ╚████║
╚═╝  ╚═══╝╚══════╝   ╚═╝   ╚═╝  ╚═╝╚══════╝ ╚═════╝ ╚═╝  ╚═╝╚═╝  ╚═══╝
```

# netrecorn · tracemap

**Cyber-recon traceroute TUI** — the whole route to a host in your terminal:
live hop table, Braille world map, ASN & geolocation for every node.

[![CI](https://github.com/sudxxt-afk/netrecorn/actions/workflows/ci.yml/badge.svg)](https://github.com/sudxxt-afk/netrecorn/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust 1.85+](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-linux%20%7C%20macOS-lightgrey.svg)](#-quick-start)
[![ratatui 0.30](https://img.shields.io/badge/TUI-ratatui%200.30-7aa2f7.svg)](https://ratatui.rs)
[![Repo size](https://img.shields.io/github/repo-size/sudxxt-afk/netrecorn)](https://github.com/sudxxt-afk/netrecorn)
[![Last commit](https://img.shields.io/github/last-commit/sudxxt-afk/netrecorn)](https://github.com/sudxxt-afk/netrecorn/commits/main)
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](https://github.com/sudxxt-afk/netrecorn/pulls)
[![Stars](https://img.shields.io/github/stars/sudxxt-afk/netrecorn?style=social)](https://github.com/sudxxt-afk/netrecorn/stargazers)

<img src="docs/demo.gif" alt="netrecorn live demo" width="900">

*Live trace: hops stream in real time, ripple rings on the map, packets marching
along the route, RTT profile under the table.*

</div>

---

## ⚡ Quick start

```bash
# build dependency: libpcap
sudo dnf install libpcap-devel        # Fedora
# sudo apt install libpcap-dev        # Debian/Ubuntu
# brew install libpcap                # macOS

cargo build --release

# raw-socket capabilities, once:
sudo setcap cap_net_raw,cap_net_admin+ep target/release/tracemap

# run from anywhere:
mkdir -p ~/.local/bin
cp target/release/tracemap ~/.local/bin/netrecorn
netrecorn google.com
```

Or simply `sudo ./target/release/tracemap google.com`.
Without privileges the app falls back to the system `traceroute`; if that also
fails, it prints a clear error with a hint.

## 🎮 Controls

| Key | Action |
|---|---|
| `↑` / `↓` (`k` / `j`) | navigate hops (selection follows the frontier until you touch the arrows) |
| `Home` / `End` | first / last hop |
| `e` | export route to `tracemap_<host>_<timestamp>.json` |
| `r` | re-run the trace |
| `q` / `Esc` | quit |

## 🧭 Features

- **Engine**: [`caracat`](https://crates.io/crates/caracat) — Paris traceroute
  over raw sockets (ICMP / ICMPv6, 3 probes per hop, up to 30 hops), streamed
  into the UI via mpsc. Fallback to system `traceroute` when unprivileged.
- **GeoIP + ASN**: ip-api.com (≤5 concurrent requests, 30-day disk cache) plus
  reverse DNS. API down → `N/A`, the UI never blocks.
- **World map**: `MapResolution::High` + Braille markers, radar graticule,
  route polylines, RTT-colored dots, magenta "you" pin, ripple on hop discovery,
  breathing nodes, marching packets, frontier pulse, blinking destination and
  an arrival ping. On wide terminals the map gets **70%** of the screen.
- **Table**: Hop / IP-Host / ASN / Location / RTT / Loss with heat colors
  (`<50ms` 🟢, `50–150` 🟡, `>150` or loss 🔴), `* * *` for silent hops,
  red private IPs, `✈` continent change, `⚠` hosting/cloud or risk-list country.
  In slim mode the selected hop's ASN/city live in a detail line under the table.
- **RTT profile**: a `▂▃▄▅▆▇█` bar per hop — the latency shape of the whole
  route at a glance.
- **Export**: stable JSON (`hop, ip, asn, country, city, lat, lon, rtt_ms,
  loss_pct`) — ready for jq and scripts.
- **Dark cyber-recon theme** out of the box: near-black background, neon cyan,
  Tokyo-Night purple/blue columns, rounded panels.

## 🖼 Screenshots

<div align="center">

| Completed route (slim table + 70% map) | Full table (wide terminal) |
|---|---|
| <img src="docs/screenshot.png" width="480"> | <img src="docs/screenshot-full.png" width="480"> |

| Trace in flight | Route across Russia (ya.ru) |
|---|---|
| <img src="docs/screenshot-run.png" width="480"> | <img src="docs/screenshot-ru.png" width="480"> |

</div>

## 🔐 Raw socket privileges

caracat injects packets and captures ICMP via libpcap → root or capabilities:

```bash
sudo setcap cap_net_raw,cap_net_admin+ep ./netrecorn   # Linux, then no sudo
sudo ./netrecorn google.com                            # macOS / lazy option
```

> Capabilities are lost on every rebuild — re-apply them after `cargo build`.

## 📦 Export format

```json
[
  {
    "hop": 8,
    "ip": "173.194.121.4",
    "asn": "AS15169",
    "country": "United States",
    "city": "Mountain View",
    "lat": 37.4056,
    "lon": -122.0775,
    "rtt_ms": 52.41,
    "loss_pct": 0.0
  }
]
```

## 🏗 Architecture

```
src/
├── main.rs      # args, ratatui init/restore, tokio select loop (keys / net / tick)
├── app.rs       # state: Vec<Hop>, selection, run lifecycle (run_id), export
├── network.rs   # caracat blocking worker + fallback, GeoIP/reverse-DNS enrichment
├── ui.rs        # header / table / RTT profile / Canvas(Map) / footer, palette
└── models.rs    # Hop, GeoInfo, NetEvent, ExportHop, continents, risk heuristics
```

The network layer sends `RunEvent { run_id, event }` over mpsc: events from an
aborted run are ignored. Blocking pcap reads live in a worker thread with
`recv_timeout` — the `pcap` crate swallows read timeouts, so this is the only
way to guarantee a per-hop deadline on silent hops.
Geo cache: `~/.cache/tracemap/geo.json`.

## 🧰 Troubleshooting

| Symptom | Fix |
|---|---|
| `Raw socket permission denied…` | `sudo` or `setcap` (see above) |
| GeoIP shows `N/A` | ip-api unreachable/rate-limited; cache fills on next run |
| Empty map | hops have no coordinates yet, or all are private/silent |
| Build fails on `pcap` | install `libpcap-devel` / `libpcap-dev` |
| `failed to initialize terminal` | run in a real terminal (TTY), not an IDE output pane |

## 🛣 Roadmap

- [ ] fullscreen map on `m`
- [ ] theme switcher (`t`): Tokyo Night / Catppuccin / Dracula / matrix
- [ ] per-hop RTT sparkline over time
- [ ] map legend on `?`
- [ ] IPv6 routes on the map (engine already speaks ICMPv6)

## 📄 License

[MIT](LICENSE) © sudxxt-afk

---

<div align="center">

# 🇷🇺 netrecorn · tracemap

**Cyber-recon traceroute TUI** — весь маршрут до хоста в терминале:
живая таблица хопов, карта мира на Braille-символах, ASN и геолокация каждого узла.

</div>

## ⚡ Быстрый старт

```bash
# зависимость сборки: libpcap
sudo dnf install libpcap-devel        # Fedora
# sudo apt install libpcap-dev        # Debian/Ubuntu
# brew install libpcap                # macOS

cargo build --release

# права на сырые сокеты — один раз:
sudo setcap cap_net_raw,cap_net_admin+ep target/release/tracemap

# запуск из любой директории:
mkdir -p ~/.local/bin
cp target/release/tracemap ~/.local/bin/netrecorn
netrecorn google.com
```

Или просто `sudo ./target/release/tracemap google.com`.
Без прав приложение само переключится на системный `traceroute`, а если и он
не может — покажет понятную ошибку с подсказкой.

## 🎮 Управление

| Клавиша | Действие |
|---|---|
| `↑` / `↓` (`k` / `j`) | навигация по хопам (выделение следует за фронтом, пока не тронешь стрелки) |
| `Home` / `End` | первый / последний хоп |
| `e` | экспорт маршрута в `tracemap_<host>_<timestamp>.json` |
| `r` | перезапустить трассировку |
| `q` / `Esc` | выход |

## 🧭 Что внутри

- **Движок**: [`caracat`](https://crates.io/crates/caracat) — Paris-traceroute
  на сырых сокетах (ICMP / ICMPv6, 3 пробы на хоп, до 30 хопов), потоково в UI
  через mpsc. Fallback на системный `traceroute` без прав.
- **GeoIP + ASN**: ip-api.com (≤5 concurrent, дисковый кэш 30 дней) + reverse DNS.
  API лёг — `N/A`, UI не блокируется никогда.
- **Карта мира**: `MapResolution::High` + Braille, радарная сетка, маршрут
  линиями, точки в цвет RTT, magenta-пин «ты», ripple при появлении хопа,
  «дышащие» узлы, бегущие пакеты, пульс фронта, мигающая цель и arrival-ping.
  На широких терминалах карта занимает **70%** экрана.
- **Таблица**: Hop / IP-Host / ASN / Location / RTT / Loss с цветовым кодом
  (`<50ms` 🟢, `50–150` 🟡, `>150` или потери 🔴), `* * *` для молчащих,
  красные private-IP, `✈` смена континента, `⚠` хостинг/облако или risk-страна.
  В slim-режиме ASN/город выбранного хопа — в строке деталей под таблицей.
- **RTT-профиль**: строка баров `▂▃▄▅▆▇█` по хопам — форма латентности маршрута
  одним взглядом.
- **Экспорт**: стабильный JSON (`hop, ip, asn, country, city, lat, lon, rtt_ms,
  loss_pct`) — готов для jq и скриптов.
- **Тёмная cyber-recon тема** из коробки: near-black фон, неоновый cyan,
  токийские фиолетовый/синий в колонках, скруглённые панели.

## 🖼 Скриншоты

<div align="center">

| Маршрут завершён (slim + карта 70%) | Полная таблица (широкий терминал) |
|---|---|
| <img src="docs/screenshot.png" width="480"> | <img src="docs/screenshot-full.png" width="480"> |

| Трассировка в полёте | Маршрут по России (ya.ru) |
|---|---|
| <img src="docs/screenshot-run.png" width="480"> | <img src="docs/screenshot-ru.png" width="480"> |

</div>

## 🔐 Права на сырые сокеты

caracat инжектит пакеты и слушает ICMP через libpcap → нужен root или capabilities:

```bash
sudo setcap cap_net_raw,cap_net_admin+ep ./netrecorn   # Linux, дальше без sudo
sudo ./netrecorn google.com                            # macOS / ленивый вариант
```

> После каждой пересборки бинарника capabilities слетают — выдай заново.

## 📦 Формат экспорта

```json
[
  {
    "hop": 8,
    "ip": "173.194.121.4",
    "asn": "AS15169",
    "country": "United States",
    "city": "Mountain View",
    "lat": 37.4056,
    "lon": -122.0775,
    "rtt_ms": 52.41,
    "loss_pct": 0.0
  }
]
```

## 🏗 Архитектура

```
src/
├── main.rs      # args, ratatui init/restore, tokio select-loop (keys / net / tick)
├── app.rs       # состояние: Vec<Hop>, selection, run lifecycle (run_id), export
├── network.rs   # caracat blocking-worker + fallback, GeoIP/reverse-DNS enrichment
├── ui.rs        # header / table / RTT-profile / Canvas(Map) / footer, палитра
└── models.rs    # Hop, GeoInfo, NetEvent, ExportHop, континенты, risk-эвристики
```

Сетевой слой шлёт `RunEvent { run_id, event }` в mpsc: события прерванного
запуска игнорируются. Блокирующие чтения pcap вынесены в воркер-поток с
`recv_timeout` — крейт `pcap` проглатывает таймауты, и это единственный способ
гарантировать deadline на молчащий хоп. Geo-кэш: `~/.cache/tracemap/geo.json`.

## 🧰 Troubleshooting

| Симптом | Лечение |
|---|---|
| `Raw socket permission denied…` | `sudo` или `setcap` (см. выше) |
| GeoIP = `N/A` | ip-api недоступен/лимит; кэш подтянет при следующем запуске |
| Карта пустая | у хопов ещё нет координат или все private/молчащие |
| Сборка падает на `pcap` | нет `libpcap-devel` / `libpcap-dev` |
| `failed to initialize terminal` | запускай в настоящем терминале (TTY), не в панели IDE |

## 🛣 Roadmap

- [ ] fullscreen-карта по `m`
- [ ] переключатель тем (`t`): Tokyo Night / Catppuccin / Dracula / matrix
- [ ] sparkline RTT выбранного хопа во времени
- [ ] легенда карты по `?`
- [ ] IPv6-маршруты на карте (движок уже умеет ICMPv6)

## 📄 Лицензия

[MIT](LICENSE) © sudxxt-afk

<div align="center">

**Made with 🦀 + [ratatui](https://ratatui.rs) · if you like it — ⭐ the repo**

</div>
