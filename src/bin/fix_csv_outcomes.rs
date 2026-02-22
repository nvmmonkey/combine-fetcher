// fix_csv_outcomes.rs — Repair corrupted market_resolved/outcome columns in CSV data files
//!
//! The multi_asset_logger had a bug where `market_resolved` and `outcome` columns
//! were sometimes empty or wrong. This binary reads CSV files, calls the Gamma API
//! to get the correct outcome for each market slug, and writes fixed copies.
//!
//! Additional fixes:
//! - Preserves valid pre-expiry outcomes detected by logger (price/WS event)
//! - Fixes last row of trailing incomplete markets (sets resolved/outcome from API)
//!
//! Usage: cargo run --bin fix_csv_outcomes -- ./221data/
//!
//! Output files are written with a `_fixed` suffix (e.g. `foo.csv` -> `foo_fixed.csv`).

use anyhow::Result;
use combine_fetcher::api::gamma;
use combine_fetcher::HTTP_TIMEOUT_SECS;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Column indices (0-based) matching CsvRecord::csv_header()
const COL_MARKET_SLUG: usize = 1;
const COL_TIME_TO_EXPIRY_MINUTES: usize = 13;
const COL_MARKET_RESOLVED: usize = 20;
const COL_OUTCOME: usize = 21;
const EXPECTED_MIN_COLS: usize = 52; // CSV has 52 columns (indices 0..51)

/// Recursively find all `.csv` files under `dir`, excluding `_fixed.csv` files.
fn find_csv_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !dir.is_dir() {
        anyhow::bail!("{} is not a directory", dir.display());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(find_csv_files(&path)?);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".csv") && !name.ends_with("_fixed.csv") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Extract the slug from a CSV data line. Returns None if the line is malformed.
fn extract_slug(line: &str) -> Option<&str> {
    let mut cols = line.splitn(COL_MARKET_SLUG + 2, ',');
    // skip column 0
    cols.next()?;
    cols.next()
}

/// Parse time_to_expiry_minutes from a CSV data line.
fn extract_ttx(fields: &[&str]) -> Option<f64> {
    if fields.len() <= COL_TIME_TO_EXPIRY_MINUTES {
        return None;
    }
    fields[COL_TIME_TO_EXPIRY_MINUTES].trim().parse::<f64>().ok()
}

/// Build the output path: `foo.csv` -> `foo_fixed.csv`
fn fixed_path(original: &Path) -> PathBuf {
    let stem = original.file_stem().unwrap_or_default().to_string_lossy();
    let parent = original.parent().unwrap_or(Path::new("."));
    parent.join(format!("{}_fixed.csv", stem))
}

/// Process a single CSV file: read rows, group by slug, fix resolution columns.
async fn process_file(
    http: &reqwest::Client,
    path: &Path,
    outcome_cache: &mut HashMap<String, String>,
) -> Result<FileStats> {
    let content = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = content.lines().collect();

    if lines.is_empty() {
        return Ok(FileStats {
            total_rows: 0,
            slugs_found: 0,
            slugs_fixed: 0,
            slugs_failed: 0,
            rows_fixed: 0,
        });
    }

    // First line is the header
    let header = lines[0];
    let data_lines = &lines[1..];

    // Collect unique slugs from this file
    let mut slugs_in_file: Vec<String> = Vec::new();
    {
        let mut seen = HashSet::new();
        for line in data_lines {
            if let Some(slug) = extract_slug(line) {
                if seen.insert(slug.to_string()) {
                    slugs_in_file.push(slug.to_string());
                }
            }
        }
    }

    let total_rows = data_lines.len();
    let slugs_found = slugs_in_file.len();
    let mut slugs_fixed = 0usize;
    let mut slugs_failed = 0usize;
    let mut rows_fixed = 0usize;

    // Fetch outcomes for all slugs (using cache to avoid redundant API calls)
    for slug in &slugs_in_file {
        if outcome_cache.contains_key(slug) {
            continue;
        }
        match gamma::fetch_outcome(http, slug).await {
            Ok(outcome) => {
                println!("    [OK] {} -> {}", slug, outcome);
                outcome_cache.insert(slug.clone(), outcome);
            }
            Err(e) => {
                eprintln!("    [WARN] {} -> Gamma API failed: {} (skipping)", slug, e);
                // Do NOT insert into cache — we skip this slug entirely
            }
        }
        // Small delay to be polite to the API
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Build the fixed output lines
    let mut output_lines: Vec<String> = Vec::with_capacity(total_rows + 1);
    output_lines.push(header.to_string());

    // For each slug group, we need to determine which rows should be marked resolved.
    // Strategy: iterate rows in order. For each row, if we have an outcome for its slug
    // and time_to_expiry_minutes <= 0, mark resolved=true and outcome=YES/NO.
    // Otherwise, ensure resolved=false and outcome is empty.
    for line in data_lines {
        let fields: Vec<&str> = line.split(',').collect();

        if fields.len() < EXPECTED_MIN_COLS {
            // Malformed row — preserve as-is
            output_lines.push(line.to_string());
            continue;
        }

        let slug = fields[COL_MARKET_SLUG];
        let ttx = extract_ttx(&fields);

        // Check if we have a cached outcome for this slug
        let cached_outcome = outcome_cache.get(slug);

        if let Some(outcome) = cached_outcome {
            let is_expired = ttx.map(|t| t <= 0.3).unwrap_or(false);

            let current_resolved = fields[COL_MARKET_RESOLVED].trim();
            let current_outcome = fields[COL_OUTCOME].trim();

            let (new_resolved, new_outcome) = if is_expired {
                ("true", outcome.as_str())
            } else if !current_outcome.is_empty() {
                // Preserve valid outcome detected by logger via price/WS before expiry
                (current_resolved, current_outcome)
            } else {
                ("false", "")
            };

            // Log when we find a wrong outcome
            if !current_outcome.is_empty() && current_outcome != outcome.as_str() && is_expired {
                println!(
                    "    [FIX] found wrong outcome {}, api correct to {}  (slug: {})",
                    current_outcome, outcome, slug
                );
            }

            // Only count as "fixed" when the outcome meaningfully changed
            // (ignore resolved="" -> "false" normalization — that's cosmetic)
            let outcome_changed = current_outcome != new_outcome;
            let resolved_actually_wrong = !current_resolved.is_empty()
                && !current_resolved.eq_ignore_ascii_case(new_resolved);

            if outcome_changed || resolved_actually_wrong {
                rows_fixed += 1;
            }

            // Rebuild the line with fixed columns
            let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
            new_fields[COL_MARKET_RESOLVED] = new_resolved.to_string();
            new_fields[COL_OUTCOME] = new_outcome.to_string();
            output_lines.push(new_fields.join(","));
        } else {
            // No outcome available (API failed or slug not found) — preserve as-is
            output_lines.push(line.to_string());
        }
    }

    // Count slugs fixed vs failed
    for slug in &slugs_in_file {
        if outcome_cache.contains_key(slug) {
            slugs_fixed += 1;
        } else {
            slugs_failed += 1;
        }
    }

    // Fix trailing incomplete market slug — but ONLY if it was genuinely
    // near expiry (logger stopped mid-market). If the last slug's final row
    // has a high TTX (> 2 min), this is an hourly rotation split and the
    // slug continues in the next file — don't touch it.
    if let Some(last_slug) = slugs_in_file.last() {
        let last_slug_reached_expiry = data_lines.iter().any(|line| {
            let fields: Vec<&str> = line.split(',').collect();
            if fields.len() < EXPECTED_MIN_COLS {
                return false;
            }
            fields[COL_MARKET_SLUG] == last_slug.as_str()
                && extract_ttx(&fields).map(|t| t <= 0.3).unwrap_or(false)
        });

        if !last_slug_reached_expiry {
            // Check the last row's TTX — if it's high, this slug was split
            // by hourly rotation and continues in the next file.
            let last_row_ttx = data_lines.iter().rev()
                .find_map(|line| {
                    let fields: Vec<&str> = line.split(',').collect();
                    if fields.len() >= EXPECTED_MIN_COLS && fields[COL_MARKET_SLUG] == last_slug.as_str() {
                        extract_ttx(&fields)
                    } else {
                        None
                    }
                });

            let is_near_expiry = last_row_ttx.map(|t| t <= 2.0).unwrap_or(false);

            if is_near_expiry {
                if let Some(api_outcome) = outcome_cache.get(last_slug) {
                    if let Some(last_idx) = output_lines.iter().rposition(|line| {
                        let fields: Vec<&str> = line.split(',').collect();
                        fields.len() >= EXPECTED_MIN_COLS && fields[COL_MARKET_SLUG] == last_slug.as_str()
                    }) {
                        let fields: Vec<&str> = output_lines[last_idx].split(',').collect();
                        let mut new_fields: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
                        new_fields[COL_MARKET_RESOLVED] = "true".to_string();
                        new_fields[COL_OUTCOME] = api_outcome.clone();
                        output_lines[last_idx] = new_fields.join(",");
                        rows_fixed += 1;
                        println!(
                            "    [TAIL-FIX] Set last row resolved=true, outcome={} for incomplete trailing market: {} (ttx={:.1}m)",
                            api_outcome, last_slug, last_row_ttx.unwrap_or(0.0)
                        );
                    }
                }
            } else if let Some(ttx) = last_row_ttx {
                println!(
                    "    [SKIP] Trailing slug {} has ttx={:.1}m — hourly rotation split, continues in next file",
                    last_slug, ttx
                );
            }
        }
    }

    // Write the fixed file
    let out_path = fixed_path(path);
    let output_content = output_lines.join("\n");
    // Ensure trailing newline if original had one
    let final_content = if content.ends_with('\n') {
        format!("{}\n", output_content)
    } else {
        output_content
    };
    std::fs::write(&out_path, final_content)?;

    Ok(FileStats {
        total_rows,
        slugs_found,
        slugs_fixed,
        slugs_failed,
        rows_fixed,
    })
}

struct FileStats {
    total_rows: usize,
    slugs_found: usize,
    slugs_fixed: usize,
    slugs_failed: usize,
    rows_fixed: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("./221data/");

    let dir_path = Path::new(dir);
    if !dir_path.exists() {
        eprintln!("Error: directory '{}' does not exist", dir);
        std::process::exit(1);
    }

    println!("=== fix_csv_outcomes ===");
    println!("Scanning: {}", dir_path.display());
    println!();

    let csv_files = find_csv_files(dir_path)?;
    if csv_files.is_empty() {
        println!("No CSV files found in {}", dir_path.display());
        return Ok(());
    }
    println!("Found {} CSV files\n", csv_files.len());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    // Global outcome cache: slug -> "YES"/"NO"
    // Since the same slug may appear in multiple files (e.g., hourly rotation),
    // caching avoids redundant API calls.
    let mut outcome_cache: HashMap<String, String> = HashMap::new();

    let mut total_files = 0usize;
    let mut total_rows_fixed = 0usize;
    let mut total_slugs_fixed = 0usize;
    let mut total_slugs_failed = 0usize;
    for (i, csv_path) in csv_files.iter().enumerate() {
        let filename = csv_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        println!(
            "[{}/{}] Processing: {}",
            i + 1,
            csv_files.len(),
            filename
        );

        match process_file(&http, csv_path, &mut outcome_cache).await {
            Ok(stats) => {
                println!(
                    "  -> {} rows, {} slug(s), {} fixed, {} failed, {} rows updated",
                    stats.total_rows,
                    stats.slugs_found,
                    stats.slugs_fixed,
                    stats.slugs_failed,
                    stats.rows_fixed,
                );
                if stats.rows_fixed > 0 {
                    println!("  -> Written: {}", fixed_path(csv_path).display());
                } else {
                    println!("  -> No changes needed (fixed file still written)");
                }
                total_files += 1;
                total_rows_fixed += stats.rows_fixed;
                total_slugs_fixed += stats.slugs_fixed;
                total_slugs_failed += stats.slugs_failed;
            }
            Err(e) => {
                eprintln!("  -> ERROR: {} (skipping file)", e);
            }
        }
        println!();
    }

    println!("=== Summary ===");
    println!("Files processed:    {}", total_files);
    println!("Unique slugs:       {} cached outcomes", outcome_cache.len());
    println!("Slugs fixed:        {}", total_slugs_fixed);
    println!("Slugs failed:       {}", total_slugs_failed);
    println!("Total rows updated: {}", total_rows_fixed);
    println!();
    println!("Outcome cache:");
    let mut cache_entries: Vec<_> = outcome_cache.iter().collect();
    cache_entries.sort_by_key(|(slug, _)| (*slug).clone());
    for (slug, outcome) in &cache_entries {
        println!("  {} -> {}", slug, outcome);
    }

    Ok(())
}
