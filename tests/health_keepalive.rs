//! #1130 — keep-alive contract between the Rust `/health` route, the
//! Cloudflare Worker cron handler, and the shared path constant.
//!
//! Contributors cannot edit `cloudflare/` or `wrangler.jsonc` (protected),
//! so this test locks the expected path by reading those files plus
//! `scripts/health_path.ts` and failing if they drift apart.

fn health_path_from_constant_file() -> String {
    let src = std::fs::read_to_string("scripts/health_path.ts")
        .expect("scripts/health_path.ts must exist");
    for line in src.lines() {
        let line = line.trim();
        if line.starts_with("export const HEALTH_CHECK_PATH") {
            let start = line.find('"').expect("path string");
            let end = line.rfind('"').expect("path string end");
            return line[start + 1..end].to_string();
        }
    }
    panic!("HEALTH_CHECK_PATH not found in scripts/health_path.ts");
}

#[test]
fn shared_health_path_constant_is_slash_health() {
    assert_eq!(health_path_from_constant_file(), "/health");
}

#[test]
fn rust_server_registers_the_shared_health_path() {
    let path = health_path_from_constant_file();
    let lib = std::fs::read_to_string("src/lib.rs").expect("src/lib.rs");
    assert!(
        lib.contains(&format!("\"{path}\"")),
        "src/lib.rs must mount the shared health path {path:?}"
    );
}

#[test]
fn worker_cron_handler_calls_the_shared_health_path() {
    // Equivalent to a test living next to cloudflare/worker.ts — verifies the
    // scheduled handler fetches the canonical keep-alive path.
    let path = health_path_from_constant_file();
    let worker = std::fs::read_to_string("cloudflare/worker.ts").expect("cloudflare/worker.ts");

    assert!(
        worker.contains("async scheduled"),
        "worker must export a scheduled (cron) handler"
    );
    assert!(
        worker.contains(&format!("container.internal{path}")),
        "cron handler must fetch http://container.internal{path}"
    );
}

#[test]
fn wrangler_cron_runs_every_five_minutes() {
    let wrangler = std::fs::read_to_string("wrangler.jsonc").expect("wrangler.jsonc");
    let constant = std::fs::read_to_string("scripts/health_path.ts").unwrap();
    assert!(
        wrangler.contains("*/5 * * * *"),
        "wrangler.jsonc must keep the five-minute keep-alive cron"
    );
    assert!(
        constant.contains("*/5 * * * *"),
        "scripts/health_path.ts HEALTH_KEEPALIVE_CRON must stay aligned with wrangler"
    );
}
