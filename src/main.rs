mod catalog;
mod lob;
mod page;
mod record;
mod sqlite;
mod types;

use crate::catalog::{extract_row_values, DatabaseCatalog, UserTablePlan};
use crate::page::MdfStorage;
use crate::record::ParsedRecord;
use crate::sqlite::{
    build_create_table_sql_with_fks, build_insert_sql, create_table_index, resolve_column_names,
    update_existing_table_ddl, SqliteDb,
};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ConversionStats {
    pub db_name: String,
    pub dbi_version: u16,
    pub tables_converted: usize,
    pub indexes_converted: usize,
    pub fks_converted: usize,
    pub rows_converted: u64,
}

const WAL_COMMIT_BATCH_ROWS: u64 = 50_000;

pub fn convert_mdf_file(
    mdf_path: &Path,
    sqlite_path: &Path,
    indexes_only: bool,
) -> Result<ConversionStats> {
    if let Some(parent) = sqlite_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create destination directory {:?}", parent)
        })?;
    }

    let storage = MdfStorage::open(mdf_path)?;
    let catalog = DatabaseCatalog::load(&storage)?;

    let db = if indexes_only && sqlite_path.exists() {
        SqliteDb::open_existing(sqlite_path)?
    } else {
        SqliteDb::create(sqlite_path)?
    };

    // Detect duplicate table names (case-insensitive in SQLite)
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for t in &catalog.tables {
        *name_counts.entry(t.table_name.to_lowercase()).or_insert(0) += 1;
    }

    let mut used_sqlite_names: HashMap<String, usize> = HashMap::new();
    let mut resolved_tables: Vec<(String, UserTablePlan)> = Vec::with_capacity(catalog.tables.len());
    let mut all_tables_by_id: HashMap<i32, (String, HashMap<i32, String>)> =
        HashMap::with_capacity(catalog.tables.len());

    for table in &catalog.tables {
        let has_dup = name_counts
            .get(&table.table_name.to_lowercase())
            .copied()
            .unwrap_or(0)
            > 1;

        let mut target_table = table.clone();
        let candidate = target_table.sqlite_table_name(has_dup);
        let lower_cand = candidate.to_lowercase();
        let count = used_sqlite_names.entry(lower_cand).or_insert(0);
        *count += 1;
        if *count > 1 {
            target_table.table_name =
                format!("{}_{}", target_table.table_name, target_table.object_id);
        }

        let sqlite_name = target_table.sqlite_table_name(has_dup);
        used_sqlite_names.insert(sqlite_name.to_lowercase(), 1);
        let col_map = resolve_column_names(&target_table.columns);
        all_tables_by_id.insert(target_table.object_id, (sqlite_name.clone(), col_map));
        resolved_tables.push((sqlite_name, target_table));
    }

    // Phase 1: Create all table schemas (including FOREIGN KEY constraints) upfront and commit
    // immediately so concurrent WAL readers can inspect the full schema and FKs right away.
    let mut total_fks: usize = 0;
    if !indexes_only {
        db.exec_batch("BEGIN TRANSACTION;")?;
        for (sqlite_name, target_table) in &resolved_tables {
            let (create_sql, fk_cnt) = build_create_table_sql_with_fks(
                sqlite_name,
                &target_table.columns,
                &target_table.foreign_keys,
                &all_tables_by_id,
            );
            db.exec_batch(&create_sql)?;
            total_fks += fk_cnt;
        }
        db.exec_batch("COMMIT;")?;
    } else {
        // Update existing SQLite table DDLs in-place to include FOREIGN KEY definitions
        db.exec_batch("PRAGMA writable_schema = ON; BEGIN TRANSACTION;")?;
        for (sqlite_name, target_table) in &resolved_tables {
            let (create_sql, fk_cnt) = build_create_table_sql_with_fks(
                sqlite_name,
                &target_table.columns,
                &target_table.foreign_keys,
                &all_tables_by_id,
            );
            if fk_cnt > 0 {
                update_existing_table_ddl(&db, sqlite_name, &create_sql)?;
                total_fks += fk_cnt;
            }
        }
        db.exec_batch("COMMIT; PRAGMA writable_schema = RESET;")?;
    }

    // Also count global index name occurrences across the database so we only prefix with
    // table name when an index name is duplicated across multiple tables or collides with a table.
    let mut idx_name_counts: HashMap<String, usize> = HashMap::new();
    for (_, t) in &resolved_tables {
        for idx in &t.indexes {
            *idx_name_counts.entry(idx.name.to_lowercase()).or_insert(0) += 1;
        }
    }

    // Phase 2: Populate table rows in WAL mode with incremental commits per table and every
    // WAL_COMMIT_BATCH_ROWS rows, then build all indexes for each table.
    let mut total_rows: u64 = 0;
    let mut total_indexes: usize = 0;
    let mut row_buf = Vec::new();

    for (sqlite_name, target_table) in &resolved_tables {
        if !indexes_only && !target_table.partitions.is_empty() {
            let insert_sql = build_insert_sql(sqlite_name, target_table.columns.len());
            let mut stmt = db.prepare(&insert_sql)?;

            db.exec_batch("BEGIN TRANSACTION;")?;
            let mut uncommitted_in_table: u64 = 0;

            for part in &target_table.partitions {
                let prefer_linked = part.rowset.idminor == 1;
                let data_pages = storage.collect_partition_data_pages(
                    part.alloc_unit.pg_first,
                    part.alloc_unit.pg_root,
                    part.alloc_unit.pg_first_iam,
                    prefer_linked,
                );

                for page_ptr in data_pages {
                    let Some(page) = storage.get_page(page_ptr) else {
                        continue;
                    };
                    for slot in 0..page.header.slot_count {
                        let Some(raw) = page.slot_slice(slot) else {
                            continue;
                        };
                        let Some(rec) = ParsedRecord::parse(raw) else {
                            continue;
                        };

                        extract_row_values(
                            &storage,
                            &rec,
                            &target_table.columns,
                            part,
                            catalog.boot.default_collation_id,
                            &mut row_buf,
                        );
                        stmt.insert_row(&row_buf)?;
                        total_rows += 1;
                        uncommitted_in_table += 1;

                        if uncommitted_in_table >= WAL_COMMIT_BATCH_ROWS {
                            db.exec_batch("COMMIT; BEGIN TRANSACTION;")?;
                            uncommitted_in_table = 0;
                        }
                    }
                }
            }

            db.exec_batch("COMMIT;")?;
        }

        // Phase 3 (per table): Create Clustered and Non-Clustered indexes in SQLite3
        if !target_table.indexes.is_empty() {
            db.exec_batch("BEGIN TRANSACTION;")?;
            for idx in &target_table.indexes {
                let lower_idx = idx.name.to_lowercase();
                let is_dup_idx = idx_name_counts.get(&lower_idx).copied().unwrap_or(0) > 1;
                let collides = used_sqlite_names.contains_key(&lower_idx);

                let mut sqlite_idx_name = if is_dup_idx || collides {
                    format!("{}_{}", sqlite_name, idx.name)
                } else {
                    idx.name.clone()
                };

                let lower_final = sqlite_idx_name.to_lowercase();
                if used_sqlite_names.contains_key(&lower_final) {
                    sqlite_idx_name =
                        format!("{}_{}_{}", sqlite_name, idx.name, idx.index_id);
                }
                used_sqlite_names.insert(sqlite_idx_name.to_lowercase(), 1);

                if create_table_index(
                    &db,
                    sqlite_name,
                    &target_table.columns,
                    idx,
                    &sqlite_idx_name,
                )? {
                    total_indexes += 1;
                }
            }
            db.exec_batch("COMMIT;")?;
        }
    }

    // Flush WAL frames into the main .sqlite3 file while keeping journal_mode = WAL
    let _ = db.exec_batch("PRAGMA wal_checkpoint(TRUNCATE);");

    Ok(ConversionStats {
        db_name: catalog.boot.database_name,
        dbi_version: catalog.boot.dbi_version,
        tables_converted: catalog.tables.len(),
        indexes_converted: total_indexes,
        fks_converted: total_fks,
        rows_converted: total_rows,
    })
}

fn discover_mdf_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if ft.is_file() {
            if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mdf")) {
                files.push(path);
            }
        } else if ft.is_dir() {
            subdirs.push(path);
        }
    }
    subdirs.sort();
    for subdir in subdirs {
        discover_mdf_files_recursive(&subdir, files);
    }
}

fn discover_mdf_files(source_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    discover_mdf_files_recursive(source_dir, &mut files);
    files.sort();
    files
}

fn resolve_single_file_dest(source_file: &Path, dest_arg: Option<&Path>) -> PathBuf {
    let stem = source_file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());

    let Some(dest_path) = dest_arg else {
        return PathBuf::from(format!("{}.sqlite3", stem));
    };

    let dest_str = dest_path.to_string_lossy();
    let ends_with_sep = dest_str.ends_with('/') || dest_str.ends_with('\\');

    if dest_path.is_dir() || ends_with_sep {
        dest_path.join(format!("{}.sqlite3", stem))
    } else if dest_path.extension().is_some() {
        dest_path.to_path_buf()
    } else {
        dest_path.join(format!("{}.sqlite3", stem))
    }
}

fn print_usage(prog: &str) {
    eprintln!(
        "Usage: {} <source_dir_or_mdf> [dest_dir_or_sqlite] [--jobs N] [--no-recursive] [--indexes-only]",
        prog
    );
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  <source_dir_or_mdf>   Source SQL Server instance directory OR a single SQL Server .mdf file");
    eprintln!("  [dest_dir_or_sqlite]  Destination directory for .sqlite3 files OR a single .sqlite3 file path");
    eprintln!("                        (default: ./<name>.sqlite3 for a single file, or ./output_sqlite for a directory)");
    eprintln!("  -j, --jobs N          Number of parallel worker threads (default: number of CPUs)");
    eprintln!("  --no-recursive        Only scan the top-level when <source_dir_or_mdf> is a directory");
    eprintln!("  --indexes-only        Only import/create indexes and foreign keys on existing .sqlite3 databases");
    eprintln!("  -h, --help            Display this help message and exit");
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let prog = args.first().map(|s| s.as_str()).unwrap_or("cobi");

    let mut positional = Vec::new();
    let mut jobs: Option<usize> = None;
    let mut recursive = true;
    let mut indexes_only = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage(prog);
                return Ok(());
            }
            "-j" | "--jobs" => {
                i += 1;
                if i >= args.len() {
                    bail!("Missing value for --jobs");
                }
                jobs = Some(args[i].parse().context("Invalid number for --jobs")?);
            }
            "--no-recursive" => {
                recursive = false;
            }
            "--indexes-only" => {
                indexes_only = true;
            }
            arg if arg.starts_with('-') => {
                bail!("Unknown option: {}", arg);
            }
            _ => {
                positional.push(PathBuf::from(&args[i]));
            }
        }
        i += 1;
    }

    if positional.is_empty() || positional.len() > 2 {
        print_usage(prog);
        bail!("Expected <source_dir_or_mdf> and optional [dest_dir_or_sqlite] arguments");
    }

    if let Some(n) = jobs {
        if n > 0 {
            let _ = rayon::ThreadPoolBuilder::new().num_threads(n).build_global();
        }
    }

    let source_path = &positional[0];
    let dest_arg = positional.get(1).map(|p| p.as_path());

    if !source_path.exists() {
        bail!("Source path {:?} does not exist", source_path);
    }

    let start_all = Instant::now();

    if source_path.is_file() {
        let out_file = resolve_single_file_dest(source_path, dest_arg);
        println!(
            "Converting single SQL Server file {:?} -> {:?}...",
            source_path, out_file
        );

        let t0 = Instant::now();
        let stats = convert_mdf_file(source_path, &out_file, indexes_only)?;
        println!(
            "[OK] {:?} -> {:?} | db={} (v{}) | tables={} | indexes={} | fks={} | rows={} | {:.2?}",
            source_path.file_name().unwrap_or_default(),
            out_file,
            stats.db_name,
            stats.dbi_version,
            stats.tables_converted,
            stats.indexes_converted,
            stats.fks_converted,
            stats.rows_converted,
            t0.elapsed()
        );
        println!(
            "\nSummary: 1/1 databases converted (0 errors) | {} tables | {} indexes | {} foreign keys | {} rows | elapsed {:.2?}",
            stats.tables_converted,
            stats.indexes_converted,
            stats.fks_converted,
            stats.rows_converted,
            start_all.elapsed()
        );
        return Ok(());
    }

    let default_dest_dir = PathBuf::from("output_sqlite");
    let dest_path = dest_arg.unwrap_or(&default_dest_dir);

    let mdf_files = if recursive {
        discover_mdf_files(source_path)
    } else {
        let mut v = Vec::new();
        for entry in std::fs::read_dir(source_path)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mdf")) {
                v.push(p);
            }
        }
        v.sort();
        v
    };

    if mdf_files.is_empty() {
        bail!("No .mdf files found in {:?}", source_path);
    }

    std::fs::create_dir_all(dest_path)?;
    println!(
        "Found {} .mdf file(s) in {:?}. Converting to {:?}...",
        mdf_files.len(),
        source_path,
        dest_path
    );

    let ok_count = AtomicUsize::new(0);
    let err_count = AtomicUsize::new(0);
    let total_tables = AtomicUsize::new(0);
    let total_indexes = AtomicUsize::new(0);
    let total_fks = AtomicUsize::new(0);
    let total_rows = AtomicU64::new(0);

    mdf_files.par_iter().for_each(|mdf_file| {
        let rel = mdf_file.strip_prefix(source_path).unwrap_or(mdf_file);
        let mut out_rel = rel.to_path_buf();
        out_rel.set_extension("sqlite3");
        let out_file = dest_path.join(out_rel);

        let t0 = Instant::now();
        match convert_mdf_file(mdf_file, &out_file, indexes_only) {
            Ok(stats) => {
                ok_count.fetch_add(1, Ordering::Relaxed);
                total_tables.fetch_add(stats.tables_converted, Ordering::Relaxed);
                total_indexes.fetch_add(stats.indexes_converted, Ordering::Relaxed);
                total_fks.fetch_add(stats.fks_converted, Ordering::Relaxed);
                total_rows.fetch_add(stats.rows_converted, Ordering::Relaxed);
                println!(
                    "[OK] {} -> {:?} | tables={} | indexes={} | fks={} | rows={} | {:.2?}",
                    rel.display(),
                    out_file.file_name().unwrap_or_default(),
                    stats.tables_converted,
                    stats.indexes_converted,
                    stats.fks_converted,
                    stats.rows_converted,
                    t0.elapsed()
                );
            }
            Err(e) => {
                err_count.fetch_add(1, Ordering::Relaxed);
                eprintln!("[ERR] {}: {:#}", rel.display(), e);
            }
        }
    });

    let ok = ok_count.load(Ordering::Relaxed);
    let err = err_count.load(Ordering::Relaxed);
    let tables = total_tables.load(Ordering::Relaxed);
    let indexes = total_indexes.load(Ordering::Relaxed);
    let fks = total_fks.load(Ordering::Relaxed);
    let rows = total_rows.load(Ordering::Relaxed);

    println!(
        "\nSummary: {}/{} databases converted ({} errors) | {} tables | {} indexes | {} foreign keys | {} rows | elapsed {:.2?}",
        ok,
        mdf_files.len(),
        err,
        tables,
        indexes,
        fks,
        rows,
        start_all.elapsed()
    );

    if err > 0 {
        bail!("{} database(s) failed to convert", err);
    }

    Ok(())
}
