# tracemap

**Cyber-recon traceroute TUI** — трассировка маршрута до хоста с живой таблицей
хопов, картой мира на Braille-символах и обогащением каждого IP через GeoIP/ASN.

```
 tracemap → google.com (142.250.185.78)        ⠹ tracing…          Hops: 8/30  ◷ 00:12
┌ Route ─────────────────────────────────────────────┐┌ Route Map ──────────────────────┐
│ Hop  IP / Host            ASN        Location      ││      .  .  .                    │
│   1  192.168.1.1          —          private net…  ││   .:::.        you              │
│   2  10.10.0.1            —          private net…  ││  ::::::.        ●───●           │
│ ✈ 3  ⚠ 172.70.1.1         AS13335    US, San Fra…  ││   :::::            \            │
│   4  142.250.185.78       AS15169    US, Mountain… ││    :::              ● (blink)   │
└────────────────────────────────────────────────────┘└─────────────────────────────────┘
 [↑/↓] Navigate  [e] Export JSON  [r] Re-run  [q] Quit   engine: caracat
```

Живой скриншот (трассировка до `1.1.1.1`, Linux + setcap):

![tracemap TUI](docs/screenshot.png)

## Возможности

- **Движок traceroute**: [`caracat`](https://crates.io/crates/caracat) — Paris
  traceroute на сырых сокетах (ICMP для IPv4, ICMPv6 для IPv6, 3 пробы на хоп,
  до 30 хопов). Если прав на сырые сокеты нет — автоматический fallback на
  системный `traceroute` с потоковым парсингом вывода.
- **Живое обновление**: хопы появляются в таблице и на карте по мере получения
  ответов, UI не блокируется (tokio + mpsc).
- **GeoIP + ASN**: для каждого публичного IP — ASN, организация, страна, город,
  координаты (ip-api.com, не более 5 одновременных запросов). Кэш на диске
  (`~/.cache/tracemap/geo.json`, TTL 30 дней) — лимиты API не превышаются.
- **Reverse DNS**: hostname вместо голого IP, когда он есть.
- **Карта мира**: `ratatui::widgets::canvas::Map` c `MapResolution::High` и
  Braille-маркерами; маршрут — линии и точки с цветом по RTT; своя точка
  (публичный IP) — magenta-пин как на картах (без подписи); анимации:
  ripple-кольцо при появлении каждого хопа, «дышащие» точки узлов,
  бегущие пакеты вдоль линии маршрута, пульсация фронта и мигающая белая
  точка на цели; подписи городов на ключевых хопах без наложений.
  На широких терминалах (≥120 колонок) карта занимает **70%** экрана:
  таблица переходит в slim-режим (`#`, IP/Host, RTT, Loss), а ASN и город
  выбранного хопа показываются строкой деталей под таблицей — контекст
  не теряется. На средних — 60%, на узких — 40% с полными колонками.
- **Цветовое кодирование RTT**: `< 50 ms` — зелёный, `50–150 ms` — жёлтый,
  `> 150 ms` или потери — красный. Молчащий хоп — `* * *` (DarkGray).
- **Разведывательные фишки**:
  - `✈` — смена континента между хопами (строка выделяется);
  - `⚠` — хостинг/облако (AWS, Google, DigitalOcean, …) или страна из
    эвристического risk-списка (`KP, IR, SY, RU, BY, CN` — правится в
    `src/models.rs`);
  - приватные IP помечаются `private network` красным.
- **Экспорт**: по `e` — JSON-файл `tracemap_<host>_<timestamp>.json`.
- **Повтор**: по `r` — полный перезапуск трассировки.

## Установка и запуск

Нужен Rust ≥ 1.85 (`rustup`) и libpcap:

```bash
# Fedora
sudo dnf install libpcap-devel
# Debian/Ubuntu
sudo apt install libpcap-dev
# macOS
brew install libpcap

cargo build --release
```

### Установка как команда `netrecorn`

Чтобы запускать из любой директории просто `netrecorn <host>`:

```bash
cargo build --release
mkdir -p ~/.local/bin
cp target/release/tracemap ~/.local/bin/netrecorn
sudo setcap cap_net_raw,cap_net_admin+ep ~/.local/bin/netrecorn
```

`~/.local/bin` входит в PATH по умолчанию на Fedora и большинстве дистрибутивов
(если нет — добавь `export PATH="$HOME/.local/bin:$PATH"` в `~/.bashrc`).
После установки: `netrecorn google.com` — без sudo, capabilities уже на бинарнике.
Имя в usage/подсказках подстраивается под имя команды, которой запустили.

### Права на сырые сокеты

caracat открывает live-capture и инжектит пакеты, поэтому нужны root или
capabilities. Три варианта:

```bash
# 1. Просто sudo
sudo ./target/release/tracemap google.com

# 2. Capabilities на бинарник (дальше можно запускать без sudo)
sudo setcap cap_net_raw,cap_net_admin+ep ./target/release/tracemap
./target/release/tracemap google.com

# 3. Без прав вообще — приложение само переключится на системный traceroute,
#    если он доступен без root (иначе покажет ошибку с подсказкой).
./target/release/tracemap google.com
```

> macOS: `setcap` не существует — используйте `sudo`.

### Запуск

```bash
tracemap <hostname|ip>
# примеры:
tracemap google.com
tracemap 1.1.1.1
tracemap ya.ru
```

Без аргумента приложение печатает usage и выходит.

## Горячие клавиши

| Клавиша     | Действие                          |
|-------------|-----------------------------------|
| `↑` / `↓` (`k` / `j`) | Навигация по таблице хопов |
| `Home` / `End`        | Первый / последний хоп     |
| `e`                   | Экспорт маршрута в JSON    |
| `r`                   | Перезапустить трассировку  |
| `q` / `Esc`           | Выход                      |

Пока идёт трассировка, выделение автоматически следует за последним хопом;
после первого нажатия стрелок управление переходит к вам.

## Формат экспорта

Массив объектов (поля стабильные):

```json
[
  {
    "hop": 3,
    "ip": "172.70.1.1",
    "asn": "AS13335",
    "country": "United States",
    "city": "San Francisco",
    "lat": 37.7621,
    "lon": -122.3971,
    "rtt_ms": 24.51,
    "loss_pct": 0.0
  }
]
```

## Архитектура

```
src/
├── main.rs      # args, ratatui init/restore, tokio select-loop (keys/net/tick)
├── app.rs       # состояние: Vec<Hop>, selection, run lifecycle, export
├── network.rs   # caracat (blocking thread) + system traceroute fallback,
│                # GeoIP/reverse-DNS enrichment (semaphore 5, disk cache)
├── ui.rs        # header / Table / Canvas(Map) / footer, cyber-recon палитра
└── models.rs    # Hop, GeoInfo, NetEvent, ExportHop, континенты, risk-эвристики
```

Сетевой слой шлёт события `RunEvent { run_id, event }` в mpsc-канал; `run_id`
гарантирует, что события от прерванного запуска (после `r`) игнорируются.
caracat полностью блокирующий (libpcap), поэтому весь его цикл живёт в
`spawn_blocking` и никогда не трогает async-reactor. Внутри движка блокирующие
чтения pcap вынесены в воркер-поток со std-каналом: крейт `pcap` проглатывает
таймауты чтения, и `recv_timeout` — единственный способ гарантировать
deadline на молчащий хоп.

## Troubleshooting

- **«Raw socket permission denied…»** — запустите через `sudo` или выдайте
  capabilities (см. выше). Без прав приложение пробует системный `traceroute`.
- **GeoIP показывает `N/A`** — ip-api.com недоступен/лимит; UI не блокируется,
  данные подтянутся из кэша при следующем запуске.
- **Карта пустая** — у хопов ещё нет координат (geo в полёте) либо все хопы
  приватные/молчащие.
- **Сборка падает на `pcap`** — не установлен `libpcap-devel`/`libpcap-dev`.

## Лицензия

MIT
