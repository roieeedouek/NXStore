//! Starts the Switch Library local web server and opens it in the browser.
//!
//! Usage: switch-library [--port N] [--no-open]
//! Listens on 127.0.0.1 only. Default port 8787; if it is taken, the next
//! free one up to 8796 is used.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use switch_library_lib::{server, Ctx};

const APP_DIR: &str = "switch-library";

#[tokio::main]
async fn main() {
    let mut port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let mut open_browser = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" | "-p" => port = args.next().and_then(|p| p.parse().ok()).unwrap_or_else(|| usage()),
            "--no-open" => open_browser = false,
            "--help" | "-h" => usage(),
            _ => usage(),
        }
    }

    let home = dirs::home_dir().unwrap_or_else(|| ".".into());
    let ctx = Arc::new(Ctx::new(
        dirs::cache_dir().unwrap_or_else(|| home.join(".cache")).join(APP_DIR),
        dirs::config_dir().unwrap_or_else(|| home.join(".config")).join(APP_DIR),
        dirs::download_dir().unwrap_or_else(|| home.join("Downloads")),
    ));

    let mut listener = None;
    for p in port..port.saturating_add(10) {
        match tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, p))).await {
            Ok(l) => {
                listener = Some((l, p));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
            Err(e) => {
                eprintln!("Could not listen on port {p}: {e}");
                std::process::exit(1);
            }
        }
    }
    let Some((listener, port)) = listener else {
        eprintln!("Ports {port}-{} are all in use. Pick another with --port.", port.saturating_add(9));
        std::process::exit(1);
    };

    let url = format!("http://127.0.0.1:{port}/");
    println!("Switch Library is running at {url}");
    println!("Downloads go to {}", ctx.download_dir.display());
    println!("Press Ctrl+C to stop.");
    if open_browser {
        if let Err(e) = opener::open_browser(&url) {
            eprintln!("Could not open a browser ({e}); open {url} yourself.");
        }
    }

    axum::serve(listener, server::router(ctx, port))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

fn usage() -> ! {
    eprintln!("Usage: switch-library [--port N] [--no-open]");
    std::process::exit(2);
}
