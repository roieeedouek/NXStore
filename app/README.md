# Switch Library

A local web app with two parts: a Nintendo Switch catalog browser with collection tracking, and a TorBox manager.
It's one small Rust program. It serves the page on `127.0.0.1` and opens it in your browser, with no installer and no window of its own.

```bash
switch-library            # starts on http://127.0.0.1:8787 and opens the browser
switch-library --port 9000
switch-library --no-open  # just print the URL
```

If port 8787 is taken, it tries the next free port up to 8796. Stop it with Ctrl+C.

## Library tab

- **Catalog** is the public [titledb](https://github.com/blawar/titledb) metadata (names, publishers, release dates, icons, banners, screenshots, descriptions) for 8 regions. It's slimmed and cached for 24 h, and the cached copy is used when you're offline.
- **Collection** lets you mark games as *Owned* or put them on a *Wishlist*. This is stored in your browser.
- **Browse** offers live search (by title, publisher or title ID), sorting (newest, oldest, title, publisher), a "hide demos" toggle, and *New* and *Upcoming* badges.
- **Details** shows the banner, facts, categories, a screenshot strip with a lightbox (←/→/Esc), a "View on eShop" link, and a click-to-copy title ID.

## TorBox tab

A simple client for your own [TorBox](https://torbox.app) account. It isn't connected to the library.

1. **Log in** by pasting your API key. The server checks it against `/user/me` and keeps it in `torbox.json` in the config folder (`0600` on Unix). The page never gets it back. **Log out** deletes the file.
2. **Add** a magnet link (sent to `torrents/createtorrent`) or an http(s) URL (sent to `webdl/createwebdownload`). Web links have to come from one of TorBox's supported hosts.
3. **Status**: torrents and web downloads are listed together, with state, progress, size, speed and ETA. The list refreshes every 4 s while anything is processing and every 20 s when idle.
4. **Download**: when an item is ready, the server requests a link (`requestdl`, zipped when there are several files) and saves it to your Downloads folder, with live progress. It writes to a `.part` file first and never overwrites an existing file. **Show in folder** opens the finished file's location.
5. **Remove** deletes an item from TorBox (click twice to confirm).

## Where files live

| | Windows | macOS | Linux |
|---|---|---|---|
| Catalog cache | `%LOCALAPPDATA%\switch-library` | `~/Library/Caches/switch-library` | `~/.cache/switch-library` |
| TorBox key | `%APPDATA%\switch-library\torbox.json` | `~/Library/Application Support/switch-library/torbox.json` | `~/.config/switch-library/torbox.json` |
| Downloads | your Downloads folder | your Downloads folder | your Downloads folder |

## Security

The server binds to loopback only. It also refuses requests whose `Host` isn't `127.0.0.1`, `localhost` or `[::1]`, which blocks DNS rebinding. API calls must send `X-Switch-Library: 1`, a custom header other websites can't add without a CORS preflight, and the server never answers those. Together these stop other sites open in your browser from using your TorBox session.

## Layout

```
app/
├── src/                  page: plain HTML/CSS/ES modules, embedded into the binary
│   ├── index.html, styles.css
│   ├── app.js            library tab and view switching
│   ├── torbox.js         TorBox tab
│   └── runtime.js        invoke() → POST /api/<cmd>, listen() → SSE /api/events
└── server/
    ├── src/main.rs       CLI, port selection, opens the browser
    ├── src/server.rs     axum routes, Host/header guard, SSE, command dispatch
    ├── src/lib.rs        catalog download/slim/cache, shared Ctx + events
    └── src/torbox.rs     TorBox client
```

## Build

```bash
cd app/server
cargo build --release                                   # native
cargo build --release --target x86_64-pc-windows-gnu    # Windows from Linux (needs gcc-mingw-w64-x86-64)
cargo test
TORBOX_API_KEY=... cargo test live -- --ignored --nocapture   # optional live round trip
```

The live test adds a small public archive.org file to the account, waits until it's ready, fetches its link, then removes it. To use a different link, set `TORBOX_TEST_LINK`.
