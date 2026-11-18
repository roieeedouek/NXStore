# Switch Library

A desktop app for browsing the Nintendo Switch catalog and tracking your collection.
It's built with Tauri v2, like the original v1.0.8 binary (see `../release/v1.0.8/`), but the code here is new.

- **Catalog** is the public [titledb](https://github.com/blawar/titledb) metadata (names, publishers, release dates, icons, banners, screenshots, descriptions) for 8 regions. It's slimmed and cached for 24 h in the app cache folder, and the cached copy is used when you're offline.
- **Collection** lets you mark games as *Owned* or put them on a *Wishlist*. This is stored locally and nothing leaves your machine.
- **Browse** offers live search (by title, publisher or title ID), sorting (newest, oldest, title, publisher), a "hide demos" toggle, and *New* and *Upcoming* badges.
- **Details** shows the banner, facts, categories, a screenshot strip with a lightbox (←/→/Esc), a "View on eShop" link, and a click-to-copy title ID.

The game library has no download or mirror features.

### TorBox tab

The TorBox tab is a separate, simple client for your own [TorBox](https://torbox.app) account. It isn't connected to the game library.

1. **Log in** by pasting your API key. The app checks it against `/user/me`. The key stays in the Rust backend and in `torbox.json` in the app config folder (`0600` on Unix), and the webview never gets it back. **Log out** deletes the file.
2. **Add** a magnet link (sent to `torrents/createtorrent`) or an http(s) URL (sent to `webdl/createwebdownload`).
3. **Status**: torrents and web downloads are listed together, with state, progress, size, speed and ETA. The list refreshes every 4 s while anything is still processing, every 20 s when everything is idle, and pauses while the tab is hidden.
4. **Download**: when an item is ready, the app requests a link (`requestdl`, zipped when there are several files) and saves it to your Downloads folder, with progress. It writes to a `.part` file first and never overwrites an existing file. **Show in folder** opens the finished file's location.
5. **Remove** deletes an item from TorBox (click twice to confirm).

## Layout

```
app/
├── src/                  frontend: plain HTML/CSS/ES module, no build step
│   ├── index.html
│   ├── styles.css
│   ├── app.js
│   └── torbox.js         TorBox tab
└── src-tauri/
    ├── src/lib.rs        commands: fetch_catalog(region, force), regions()
    ├── src/torbox.rs     torbox_login/session/logout/add/list/remove/download
    ├── tauri.conf.json
    └── capabilities/     opener limited to nintendo.com / github.com / torbox.app,
                          plus revealing files in Downloads
```

## Develop

Prerequisites: Rust, Node 18+, and on Linux `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev`.

```bash
cd app
npm install
npm run dev      # run with hot reload of the frontend
npm run build    # AppImage / NSIS / .app / .dmg into src-tauri/target/release/bundle
cd src-tauri && cargo test
```

Before the first build, generate the platform icons from the source PNG:

```bash
npx tauri icon src-tauri/icons/icon.png
```

## Not done yet

- **Self-update:** the plan is to add `tauri-plugin-updater` with a signing key, instead of the original's hand-rolled script swap.
- **CI:** a GitHub Actions workflow using `tauri-apps/tauri-action` to build all platforms.
- **Library folder scanning:** match your own dumps by title ID to fill in "Owned" automatically.
