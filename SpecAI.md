# SpecAI - Cobi Project (SQL Server .MDF to SQLite3 Converter in Rust)

> **Language Policy**: This specification file (`SpecAI.md`) MUST always be written and maintained in **English**.

## 1. Project Purpose
`Cobi` is a standalone command-line tool written in **Rust** designed to read Microsoft SQL Server primary database binary files (`.mdf`) directly offline (without any running SQL Server engine, on both Linux and Windows) and convert either an entire SQL Server instance directory or a single `.mdf` file into self-contained **SQLite3** (`.sqlite3`) databases, preserving the relative subdirectory hierarchy.

Its primary goal is to provide complete, immediate access to **user table schemas**, **Foreign Keys**, **all table data**, and **all indexes** contained in the databases (migrated automatically by default), converting all strings (both `UTF-16LE` and single-byte/DBCS Windows codepages determined by `collation_id`) into valid **UTF-8**, and opening/populating the SQLite3 databases in **WAL (`Write-Ahead Logging`)** mode with incremental commits so they can be queried concurrently even while they are being populated.

- **License**: GNU General Public License v2.0 (GPL-2.0).

---

## 2. Code Architecture (`src/`)

1. **[`src/page.rs`](src/page.rs) — Storage Engine, Page Header, Torn Page, PFS, IAM, and B-Tree**:
   - Memory-maps `.mdf` files in read-only mode via `memmap2`.
   - Reads 8,192-byte blocks (`PAGE_SIZE`), validates the 96-byte Page Header, and **automatically restores Torn Page Detection bits (`flag_bits & 0x0100 != 0`)**:
     - When bit `0x0100` is set, bits `0..1` of `m_tornBits` (header offset `60..63`, located in Sector 0) store the 2-bit torn signature written to sectors `1..15`, while bits `2..31` of `m_tornBits` preserve the original 2 least significant bits of the last byte of each 512-byte sector for `s in 1..16` (`bytes 1023, 1535, ..., 8191`, including byte `8191` which is the high byte of Slot 0's offset!). Sector 0 (`byte 511`) is never modified by SQL Server.
   - Reads the **Slot Array** backward from the end of the page (`8192 - 2 * (i + 1)`).
   - Parses the **Boot Page `(1:9)`** extracting `dbi_version`, `dbi_createVersion`, `dbi_dbname`, `dbi_collation`, and `dbi_firstSysIndexes`.
   - Traverses **B-Trees** starting from `pgroot` (descending across Type 2 index records via Slot 0's Down-Page Pointer at `rec0[pminlen - 6 .. pminlen]`) down to the leftmost Type 1 leaf page (`DATA_PAGE`) and following the `m_nextPage` chain, with full fallback and support for **IAM (`PageType::IAM = 10`)** chain scanning (8 single-page slots + extent bitmaps verified via the `0x40` Allocated bit in **PFS** pages every 8,088 pages).

2. **[`src/record.rs`](src/record.rs) — Record Decoder (`FixedVar`, Forwarding Stub, Sparse Vector)**:
   - Decodes `PrimaryRecord` (`RecordType == 0`) and `ForwardedRecord` (`RecordType == 1`) on data pages (ignoring Type 2 `ForwardingStub` records during full partition scans so each forwarded row is visited once, while providing `resolve_slot_record` for point lookups).
   - Extracts the Fixed-Length section, column count, and **Null Bitmap** (also handling columns added via `ALTER TABLE ADD COLUMN` that are absent from historical records).
   - Extracts the **Variable-Length Offset Table**, handling the `0x8000` flag (*Complex Column*) and **Sparse Vector** (`Complex ID == 5` / `261`).

3. **[`src/lob.rs`](src/lob.rs) — LOB and Row-Overflow Reassembly**:
   - Resolves 16-byte **TextPointer** and **BlobInlineRoot / Row-Overflow** pointers (`Complex ID == 2 | 4`, array of 12-byte pointers starting at offset 12).
   - Recursively traverses LOB pages (`PageType::TextMix = 3`, `PageType::TextTree = 4`) reassembling `SMALL_ROOT (0)`, `LARGE_ROOT (1)`, `INTERNAL (2)`, `DATA (3)`, `LARGE_ROOT_SHILOH (4)`, and `LARGE_ROOT_YUKON (5)` nodes.

4. **[`src/types.rs`](src/types.rs) — SQL Server Type Decoding and UTF-8 Conversion**:
   - Supports all SQL Server types: `tinyint (48)`, `smallint (52)`, `int (56)`, `bigint (127)`, `bit (104)`, `real (59)`, `float (62)`, `smallmoney (122)`, `money (60)`, `decimal (106)`, `numeric (108)`, `date (40)`, `time (41)`, `datetime2 (42)`, `datetimeoffset (43)`, `smalldatetime (58)`, `datetime (61)`, `uniqueidentifier (36)`, `char (175)`, `varchar (167)`, `text (35)`, `nchar (239)`, `nvarchar (231)`, `ntext (99)`, `binary (173)`, `varbinary (165)`, `image (34)`, `timestamp/rowversion (189)`, `xml (241)`, and `sql_variant (98)`.
   - Maps `collation_id` (LCID and UTF-8 flag `0x0800_0000`) to the corresponding `encoding_rs` decoder (`WINDOWS_1252`, `WINDOWS_1250..1258`, `WINDOWS_874`, `SHIFT_JIS`, `GBK`, `EUC_KR`, `BIG5`, `UTF_8`), converting every single-byte/DBCS and `UTF-16LE` string into valid **UTF-8**.

5. **[`src/catalog.rs`](src/catalog.rs) — System Catalog Bootstrap, Indexes, Foreign Keys, and Physical Row Extraction**:
   - Bootstraps from `boot.first_sys_indexes`:
     - `sys.sysallocunits` (`idmajor = 7`)
     - `sys.sysrowsets` (`idmajor = 5`)
     - `sys.sysowners` (`idmajor = 44`)
     - `sys.sysschobjs` (`idmajor = 34`, extracting both user tables `type == "U "` and Foreign Key constraints `type == "F "`)
     - `sys.syscolpars` (`idmajor = 41`, `number == 0`)
     - `sys.sysrscols` (`idmajor = 3`), joining `rscolid == syscolpars.colid` to obtain exact physical offsets (`leaf_offset`, `leaf_nullbit`, `leaf_bitpos`, `status`) for every column in the record.
     - `sys.sysidxstats` (`idmajor = 54`) and `sys.siscols` (`idmajor = 55`), extracting all Clustered (`type = 1`) and Non-Clustered (`type = 2`) indexes, `is_unique` (`status & 0x08`), `is_primary_key` (`status & 0x20`), key column order (`subid` where `status & 0x01 != 0 && tp1 > 0`), and `ASC`/`DESC` sort direction (`status & 0x04`).
     - `sys.syssingleobjrefs` (`idmajor = 74`) and `sys.sysmultiobjrefs` (`idmajor = 75`), extracting for each `"F "` constraint in `sys.sysschobjs`:
       - the referenced parent table (`class == 27`, `indepid = referenced_object_id`);
       - the child table columns (`class == 28`, `depsubid = constraint_column_id`, `indepsubid = parent_column_id`);
       - the referenced parent table columns (`class == 29`, `depsubid = constraint_column_id`, `indepsubid = referenced_column_id`);
       - the `ON DELETE` (`(status >> 12) & 3`) and `ON UPDATE` (`(status >> 14) & 3`: `0 = NO ACTION`, `1 = CASCADE`, `2 = SET NULL`, `3 = SET DEFAULT`) referential actions.

6. **[`src/sqlite.rs`](src/sqlite.rs) — Native SQLite3 Writer (FFI `libsqlite3`) in WAL Mode, Foreign Keys, and Index Creation**:
   - Uses `libsqlite3` directly via C FFI.
   - Configures each database with:
     - `PRAGMA encoding = 'UTF-8';`
     - `PRAGMA locking_mode = NORMAL;`
     - `PRAGMA journal_mode = WAL;`
     - `PRAGMA synchronous = NORMAL;`
     - `PRAGMA wal_autocheckpoint = 1000;`
     - `PRAGMA busy_timeout = 5000;`
   - Creates all user tables upfront including `CONSTRAINT "..." FOREIGN KEY (...) REFERENCES "..." (...) [ON DELETE ...] [ON UPDATE ...]` clauses in an initial transaction committed immediately (making the full schema and relationships immediately visible to concurrent WAL readers), populates data committing at the end of each table and every `50,000` rows (`WAL_COMMIT_BATCH_ROWS`), and finally creates all indexes (`CREATE UNIQUE INDEX` / `CREATE INDEX` with automatic fallback on collation case-insensitivity collisions) before running `PRAGMA wal_checkpoint(TRUNCATE);`.

7. **[`src/main.rs`](src/main.rs) — Parallel CLI (Single-File or Directory Mode)**:
   - Accepts `<source_dir_or_mdf> [dest_dir_or_sqlite] [--jobs N] [--no-recursive] [--indexes-only]`.
   - **If the source is a single `.mdf` file**: converts only that SQL Server file into the specified `.sqlite3` file (or `<dest_dir>/<stem>.sqlite3` if the destination is a directory or omitted).
   - **If the source is a directory**: recursively discovers all `.mdf`/`.MDF` files and converts them in parallel into the destination directory while preserving the relative subdirectory tree.
   - Migrates schemas, foreign keys, data rows, and indexes automatically by default.

8. **[`build.rs`](build.rs) — Linux & Windows Cross-Compilation**:
   - On Linux, dynamically links the system `libsqlite3` (`-lsqlite3`).
   - On Windows (`x86_64-pc-windows-gnu`), automatically compiles `sqlite3.c` via `x86_64-w64-mingw32-gcc` and links it statically into `cobi.exe`, producing a completely self-contained x86-64 PE32+ executable (requiring only standard Win32 OS DLLs: `KERNEL32.dll`, `msvcrt.dll`, `ntdll.dll`, `bcryptprimitives.dll`).

9. **Documentation, README, and Man Page**:
   - **[`README.md`](README.md)**: Comprehensive user-facing project documentation for GitHub, outlining features, architecture, installation, build steps (Linux and Windows), and CLI usage examples.
   - **[`MDF-Format.md`](MDF-Format.md)**: Complete technical specification in English of the Microsoft SQL Server binary format (`.mdf`, `.ndf`, `.ldf`, `.bak`).
   - **[`man/cobi.1`](man/cobi.1)**: Unix manual page (`roff`), viewable via `man -l man/cobi.1` or `make man`.

---

## 3. Build and Execution
- **Linux Build**: `make release` (produces `target/release/cobi`).
- **Windows Build (x86_64)**: `make windows` (produces `target/x86_64-pc-windows-gnu/release/cobi.exe`).
- **View Man Page**: `make man` (or `man -l man/cobi.1`).
- **Start Conversion**: `./start.sh [source_dir_or_mdf] [dest_dir_or_sqlite]`
- **Stop Conversion**: `./stop.sh`

---

## 4. Progress and Verification Status
- [x] Complete implementation of `cobi` in Rust (`src/page.rs`, `src/record.rs`, `src/lob.rs`, `src/types.rs`, `src/catalog.rs`, `src/sqlite.rs`, `src/main.rs`, `build.rs`).
- [x] Creation of `Makefile`, `start.sh`, `stop.sh`, `man/cobi.1`, `MDF-Format.md` (in English), and `.gitignore`.
- [x] Support for both single `.mdf` file conversion and full SQL Server instance directory conversion.
- [x] Verified cross-compilation for Windows (`target/x86_64-pc-windows-gnu/release/cobi.exe`) with statically linked `sqlite3`.
- [x] Full verification across a complete instance directory:
  - **170 / 170 `.mdf` databases converted successfully (0 errors)** into `output_sqlite`.
  - **19,480 user tables** and **43,857,182 rows** migrated into UTF-8 SQLite3.
  - **22,145 indexes** (`CREATE UNIQUE INDEX` / `CREATE INDEX`) and **489 Foreign Keys** (`CONSTRAINT ... FOREIGN KEY ... REFERENCES ...`) extracted and migrated automatically by default.
- [x] Enabled **WAL mode (`PRAGMA journal_mode = WAL;`, `PRAGMA locking_mode = NORMAL;`)** with incremental commits so `.sqlite3` files can be queried concurrently while being populated.
