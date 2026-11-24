//! `fake-provider [--port N] [--bad-design-once-for TEXT] [--delay-ms N]`
//! Prints `LISTENING http://127.0.0.1:PORT` once ready. Test tooling only.
//!
//! When started under a name ending in `codex` (for example through a
//! symlink) it acts as a fake Codex CLI. When started with Claude Code's
//! flags (`-p` or `--version`) it acts as a fake Claude Code CLI. Tests point
//! the app at this binary in place of the real tools.

use fake_provider::{Fake, invoked_as_codex, run_fake_claude, run_fake_codex};

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.first().is_some_and(|a| invoked_as_codex(a)) {
        std::process::exit(run_fake_codex(&args));
    }
    if args.iter().any(|a| a == "-p" || a == "--version") {
        std::process::exit(run_fake_claude(&args));
    }
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let port: u16 = arg("--port").and_then(|p| p.parse().ok()).unwrap_or(0);
    let fake = Fake::new();
    fake.configure(|c| {
        c.bad_design_once_for = arg("--bad-design-once-for");
        c.delay_ms = arg("--delay-ms").and_then(|v| v.parse().ok()).unwrap_or(0);
    });
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let running = fake.serve(port).await?;
            println!("LISTENING {}", running.base);
            tokio::signal::ctrl_c().await
        })
}
