#![cfg(unix)]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiarun::apps::AppCategoryFilter;
use tiarun::config::Config;
use tiarun::icon_resolver::IconResolver;
use tiarun::launcher::{ItemType, LauncherEngine, LauncherItem};

fn get_rss_kb() -> usize {
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("VmRSS:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<usize>() {
                        return kb;
                    }
                }
            }
        }
    }
    0
}

fn calc_percentiles(mut latencies: Vec<Duration>) -> (Duration, Duration, Duration, Duration, Duration) {
    if latencies.is_empty() {
        return (Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO, Duration::ZERO);
    }
    latencies.sort();
    let sum: Duration = latencies.iter().sum();
    let avg = sum / latencies.len() as u32;
    let p50 = latencies[(latencies.len() as f64 * 0.50) as usize];
    let p95 = latencies[((latencies.len() as f64 * 0.95) as usize).min(latencies.len() - 1)];
    let p99 = latencies[((latencies.len() as f64 * 0.99) as usize).min(latencies.len() - 1)];
    let max = *latencies.last().unwrap();
    (avg, p50, p95, p99, max)
}

#[test]
#[ignore = "stress benchmark - run with: cargo test --release --test stress_test -- --ignored --nocapture"]
fn test_viewlauncher_comprehensive_stress_bench() {
    println!("\n================================================================================");
    println!("             VIEW-LAUNCHER COMPREHENSIVE PERFORMANCE STRESS TEST                ");
    println!("================================================================================\n");

    let initial_rss = get_rss_kb();
    println!("Baseline Process Memory (RSS): {:.2} MB", initial_rss as f64 / 1024.0);

    // -------------------------------------------------------------------------
    // 1. Cold Engine Startup & App Indexing on Live System
    // -------------------------------------------------------------------------
    println!("\n[1/6] Benchmarking Cold Engine Startup on Live System...");
    let start_startup = Instant::now();
    let config = Config::default();
    let mut engine = LauncherEngine::new(config);
    let startup_dur = start_startup.elapsed();
    println!("  -> Cold start completed in: {:.2} ms", startup_dur.as_secs_f64() * 1000.0);
    println!("  -> Real system apps indexed: {}", engine.apps.len());
    println!("  -> Available categories: {:?}", engine.available_categories());

    // -------------------------------------------------------------------------
    // 2. Stress Test with 10,000 Synthetic Applications
    // -------------------------------------------------------------------------
    println!("\n[2/6] Generating 10,000 Synthetic Applications catalog for stress test...");
    let app_prefixes = [
        "Visual Studio", "IntelliJ IDEA", "Postman API", "Google Chrome", "Mozilla Firefox",
        "Spotify Music", "Discord Chat", "Slack Work", "Docker Desktop", "Terminal Console",
        "VLC Media Player", "LibreOffice Writer", "DBeaver Database", "GIMP Image Editor",
        "Blender 3D", "Obsidian Notes", "Neovim Editor", "Alacritty Terminal", "Telegram Desktop",
        "Cài đặt Hệ thống", "Trình duyệt Cốc Cốc", "Bộ gõ tiếng Việt", "Quản lý tập tin"
    ];

    let mut synthetic_apps = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        let prefix = app_prefixes[i % app_prefixes.len()];
        let name = format!("{} v{}.{}", prefix, i / 100, i % 100);
        let exec = format!("/usr/bin/app_{}", i);
        let cat = match i % 6 {
            0 => "Development;IDE;",
            1 => "Network;WebBrowser;",
            2 => "AudioVideo;Player;",
            3 => "Office;Spreadsheet;",
            4 => "System;Utility;",
            _ => "Utility;",
        };
        let mut item = LauncherItem::new(
            name,
            exec,
            ItemType::App,
            Some(format!("Synthetic application description for item {}", i)),
            false,
            Some("app-icon".to_string()),
        );
        item.desktop_categories = Some(Arc::from(cat));
        synthetic_apps.push(item);
    }
    engine.apps = synthetic_apps;
    println!("  -> Successfully loaded {} apps into search engine index.", engine.apps.len());

    // -------------------------------------------------------------------------
    // 3. Search Matching Latency under High Concurrency (10,000 Apps)
    // -------------------------------------------------------------------------
    println!("\n[3/6] Stress testing search latency across 10,000 applications (1,000 queries)...");
    let test_queries = [
        "v", "vi", "vis", "visual", "int", "intel", "idea", "code", "chr", "chrome",
        "fire", "spot", "disc", "dock", "term", "lib", "dbeav", "gimp", "blen", "cài",
        "trình", "tiếng", "xyz_nonexistent_query", "a", "s", "m", "p", "o"
    ];

    let mut query_latencies = Vec::with_capacity(1_000);
    let total_search_start = Instant::now();

    for i in 0..1000 {
        let q = test_queries[i % test_queries.len()];
        let t0 = Instant::now();
        let results = engine.search(q);
        let elapsed = t0.elapsed();
        query_latencies.push(elapsed);
        assert!(!results.is_empty() || q == "xyz_nonexistent_query");
    }
    let total_search_time = total_search_start.elapsed();
    let (avg, p50, p95, p99, max) = calc_percentiles(query_latencies);
    let qps = 1000.0 / total_search_time.as_secs_f64();

    println!("  -> Total queries executed: 1,000");
    println!("  -> Search Throughput:     {:.0} queries/sec", qps);
    println!("  -> Avg Query Latency:     {:.2} ms ({:.1} µs)", avg.as_secs_f64() * 1000.0, avg.as_micros() as f64);
    println!("  -> P50 (Median) Latency:  {:.2} ms ({:.1} µs)", p50.as_secs_f64() * 1000.0, p50.as_micros() as f64);
    println!("  -> P95 Latency:           {:.2} ms ({:.1} µs)", p95.as_secs_f64() * 1000.0, p95.as_micros() as f64);
    println!("  -> P99 Latency:           {:.2} ms ({:.1} µs)", p99.as_secs_f64() * 1000.0, p99.as_micros() as f64);
    println!("  -> Max Latency:           {:.2} ms ({:.1} µs)", max.as_secs_f64() * 1000.0, max.as_micros() as f64);

    // -------------------------------------------------------------------------
    // 4. File Search Stress Test (50,000 files in Rayon index)
    // -------------------------------------------------------------------------
    println!("\n[4/6] Stress testing parallel file search across 50,000 paths...");
    {
        let mut files_guard = engine.shallow_files.write().unwrap();
        files_guard.clear();
        for i in 0..50_000 {
            let path = format!("/home/user/workspace/project_{}/src/module_{}/file_{}.rs", i / 500, i / 50, i);
            let name = format!("file_{}.rs", i);
            files_guard.push(LauncherItem::new(
                name,
                path,
                ItemType::File,
                None,
                false,
                None,
            ));
        }
    }

    let file_queries = ["@f file_12", "@f module_3", "@f project_1", "@f 999", "@f not_found"];
    let mut file_latencies = Vec::new();
    for _ in 0..100 {
        for &fq in &file_queries {
            let t0 = Instant::now();
            let _ = engine.search(fq);
            file_latencies.push(t0.elapsed());
        }
    }
    let (f_avg, f_p50, f_p95, f_p99, f_max) = calc_percentiles(file_latencies);
    println!("  -> Total file queries:    500");
    println!("  -> Indexed File Count:    50,000 items");
    println!("  -> Avg File Latency:      {:.2} ms", f_avg.as_secs_f64() * 1000.0);
    println!("  -> P50 (Median) Latency:  {:.2} ms", f_p50.as_secs_f64() * 1000.0);
    println!("  -> P95 Latency:           {:.2} ms", f_p95.as_secs_f64() * 1000.0);
    println!("  -> P99 Latency:           {:.2} ms", f_p99.as_secs_f64() * 1000.0);
    println!("  -> Max File Latency:      {:.2} ms", f_max.as_secs_f64() * 1000.0);

    // -------------------------------------------------------------------------
    // 5. Category Filtering & Icon Cache Lookup Stress Test
    // -------------------------------------------------------------------------
    println!("\n[5/6] Benchmarking Category Filtering and Icon Cache Lookup...");
    let resolver = Arc::new(IconResolver::new());
    
    // Benchmark category filtering over 10,000 apps
    let t_cat_start = Instant::now();
    let dev_filter = AppCategoryFilter::Development;
    let mut dev_count = 0;
    for app in &engine.apps {
        if dev_filter.matches(app) {
            dev_count += 1;
        }
    }
    let cat_filter_dur = t_cat_start.elapsed();
    println!("  -> Filtered 10,000 apps into Development category in: {:.2} ms (Found: {} apps)", 
        cat_filter_dur.as_secs_f64() * 1000.0, dev_count);

    // Benchmark 100,000 cached icon lookups
    let icon_lookup_start = Instant::now();
    for i in 0..100_000 {
        let key = test_queries[i % test_queries.len()];
        let _ = resolver.resolve_icon(Some(key), key, key);
    }
    let icon_dur = icon_lookup_start.elapsed();
    let icon_ops_sec = 100_000.0 / icon_dur.as_secs_f64();
    let ns_per_lookup = (icon_dur.as_nanos() as f64) / 100_000.0;
    println!("  -> Icon Cache Lookups:    100,000 operations");
    println!("  -> Icon Lookup Throughput: {:.0} ops/sec", icon_ops_sec);
    println!("  -> Avg Icon Lookup Cost:  {:.1} nanoseconds per item", ns_per_lookup);

    // -------------------------------------------------------------------------
    // 6. Live Daemon IPC Toggle Latency
    // -------------------------------------------------------------------------
    println!("\n[6/6] Benchmarking Live Daemon IPC Ping Roundtrip...");
    let socket_path = dirs::runtime_dir()
        .map(|r| r.join("view-launcher.sock"))
        .unwrap_or_else(|| PathBuf::from("/tmp/view-launcher.sock"));

    if socket_path.exists() {
        let mut ipc_latencies = Vec::new();
        for _ in 0..50 {
            let t0 = Instant::now();
            if let Ok(mut stream) = UnixStream::connect(&socket_path) {
                // Send ping
                let _ = stream.write_all(b"ping");
                ipc_latencies.push(t0.elapsed());
            }
        }
        if !ipc_latencies.is_empty() {
            let (ipc_avg, ipc_p50, ipc_p95, _, ipc_max) = calc_percentiles(ipc_latencies);
            println!("  -> IPC Socket Path:       {:?}", socket_path);
            println!("  -> Avg Toggle Latency:    {:.3} ms ({:.1} µs)", ipc_avg.as_secs_f64() * 1000.0, ipc_avg.as_micros() as f64);
            println!("  -> P50 Toggle Latency:    {:.3} ms ({:.1} µs)", ipc_p50.as_secs_f64() * 1000.0, ipc_p50.as_micros() as f64);
            println!("  -> P95 Toggle Latency:    {:.3} ms ({:.1} µs)", ipc_p95.as_secs_f64() * 1000.0, ipc_p95.as_micros() as f64);
            println!("  -> Max Toggle Latency:    {:.3} ms ({:.1} µs)", ipc_max.as_secs_f64() * 1000.0, ipc_max.as_micros() as f64);
        }
    } else {
        println!("  -> IPC Socket not detected at {:?}. Skipping live socket ping.", socket_path);
    }

    let final_rss = get_rss_kb();
    println!("\n--------------------------------------------------------------------------------");
    println!("Final Memory (RSS) with 10k apps & 50k files in RAM: {:.2} MB (Delta: +{:.2} MB)",
        final_rss as f64 / 1024.0, (final_rss.saturating_sub(initial_rss)) as f64 / 1024.0);
    println!("================================================================================\n");
}
