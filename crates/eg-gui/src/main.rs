//! `eg-gui` — ExcelGRAG with a face.
//!
//! A local web server over an ExcelGRAG corpus: the engine's own crates,
//! in-process, behind a REST API and one WebSocket. Serves the built
//! frontend from the same port, so the whole GUI is one `eg-gui <corpus>`
//! and a browser tab.
//!
//! ```sh
//! eg-gui corpus/                 # serve the GUI on 127.0.0.1:8765
//! eg-gui corpus/ --port 9000     # elsewhere
//! eg-gui corpus/ --open          # and open the browser at it
//! ```
//!
//! Binds to localhost only, always: a corpus is someone's spreadsheet, and
//! the GUI must not become the thing that puts it on a network.

mod api;
mod app;
mod dto;
mod index_job;
mod watch;

use std::sync::Arc;

use clap::Parser;

use app::App;

#[derive(Parser)]
#[command(
    name = "eg-gui",
    version,
    about = "A local web UI over an ExcelGRAG corpus: live, interactive graph views."
)]
struct Cli {
    /// The corpus directory. Created if absent — index from the UI.
    dir: String,
    /// Port to serve on, on localhost only.
    #[arg(long, default_value_t = 8765)]
    port: u16,
    /// Show the kind of each value rather than the value, wherever the GUI
    /// would show a cell. Matches `eg serve --redact-values`.
    #[arg(long)]
    redact_values: bool,
    /// Open the browser at the served URL.
    #[arg(long)]
    open: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let app = Arc::new(App::new(&cli.dir, cli.redact_values).map_err(anyhow::Error::msg)?);

    // A corpus the engine just created is an ordinary starting state — the
    // GUI can index from scratch — but it deserves a sentence, because the
    // person who typed the wrong directory is also starting at an empty GUI.
    {
        let state = app.engine();
        if state.corpus.is_empty() {
            eprintln!(
                "eg-gui: the corpus at {} is empty — index a workbook from the UI, \
                 or with `eg index {} <workbook>`",
                cli.dir, cli.dir
            );
        }
    }

    if let Err(e) = watch::spawn(Arc::clone(&app)) {
        // Liveness is a feature, not a prerequisite: a corpus on a filesystem
        // notify cannot watch still serves; it just doesn't update itself.
        tracing::warn!("not watching the corpus for changes: {e}");
    }

    let url = format!("http://127.0.0.1:{}", cli.port);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", cli.port)).await?;
    println!("eg-gui serving {} — {}", app.dir, url);
    if cli.open {
        let _ = webbrowser::open(&url);
    }

    axum::serve(listener, api::router(app)).await?;
    Ok(())
}
