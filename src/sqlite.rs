use crate::catalog::{ColumnDef, ForeignKeyDef, IndexDef, UserTablePlan};
use crate::types::{sql_type_to_sqlite_decl, SqliteValue};
use anyhow::{bail, Result};
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;
use std::ptr;

#[repr(C)]
pub struct sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
pub struct sqlite3_stmt {
    _private: [u8; 0],
}

const SQLITE_OK: c_int = 0;
const SQLITE_DONE: c_int = 101;

type SqliteDestructor = Option<unsafe extern "C" fn(*mut c_void)>;

fn sqlite_transient() -> SqliteDestructor {
    unsafe { std::mem::transmute(-1isize) }
}

extern "C" {
    fn sqlite3_open(filename: *const c_char, pp_db: *mut *mut sqlite3) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        z_sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut sqlite3_stmt,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_reset(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_clear_bindings(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, idx: c_int) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, idx: c_int, val: i64) -> c_int;
    fn sqlite3_bind_double(stmt: *mut sqlite3_stmt, idx: c_int, val: f64) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        idx: c_int,
        val: *const c_char,
        n: c_int,
        destructor: SqliteDestructor,
    ) -> c_int;
    fn sqlite3_bind_blob(
        stmt: *mut sqlite3_stmt,
        idx: c_int,
        val: *const c_void,
        n: c_int,
        destructor: SqliteDestructor,
    ) -> c_int;
}

pub struct SqliteDb {
    db: *mut sqlite3,
}

impl SqliteDb {
    pub fn create<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        if path_ref.exists() {
            std::fs::remove_file(path_ref)?;
        }
        let path_str = path_ref.to_string_lossy();
        let wal_path = format!("{}-wal", path_str);
        let shm_path = format!("{}-shm", path_str);
        let _ = std::fs::remove_file(&wal_path);
        let _ = std::fs::remove_file(&shm_path);
        Self::open_existing(path_ref)
    }

    pub fn open_existing<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_ref = path.as_ref();
        let path_str = path_ref.to_string_lossy();
        let c_path = CString::new(path_str.as_bytes())?;
        let mut db: *mut sqlite3 = ptr::null_mut();
        let rc = unsafe { sqlite3_open(c_path.as_ptr(), &mut db) };
        if rc != SQLITE_OK || db.is_null() {
            let msg = if !db.is_null() {
                unsafe { CStr::from_ptr(sqlite3_errmsg(db)).to_string_lossy().into_owned() }
            } else {
                format!("sqlite3_open failed with code {}", rc)
            };
            if !db.is_null() {
                unsafe {
                    sqlite3_close(db);
                }
            }
            bail!("Failed to open SQLite database {:?}: {}", path_ref, msg);
        }

        let wrapper = Self { db };
        wrapper.exec_batch(
            "PRAGMA encoding = 'UTF-8';
             PRAGMA locking_mode = NORMAL;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA wal_autocheckpoint = 1000;
             PRAGMA busy_timeout = 5000;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -32000;",
        )?;
        Ok(wrapper)
    }

    pub fn last_error(&self) -> String {
        unsafe {
            let ptr = sqlite3_errmsg(self.db);
            if ptr.is_null() {
                "Unknown SQLite error".to_string()
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    pub fn exec_batch(&self, sql: &str) -> Result<()> {
        let c_sql = CString::new(sql)?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            sqlite3_exec(
                self.db,
                c_sql.as_ptr(),
                None,
                ptr::null_mut(),
                &mut errmsg,
            )
        };
        if rc != SQLITE_OK {
            let err_str = if !errmsg.is_null() {
                let s = unsafe { CStr::from_ptr(errmsg).to_string_lossy().into_owned() };
                unsafe {
                    sqlite3_free(errmsg as *mut c_void);
                }
                s
            } else {
                self.last_error()
            };
            bail!("SQLite exec error ({}): {} [SQL: {}]", rc, err_str, sql);
        }
        Ok(())
    }

    pub fn prepare(&self, sql: &str) -> Result<SqliteStmt<'_>> {
        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.db,
                sql.as_ptr() as *const c_char,
                sql.len() as c_int,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            bail!(
                "SQLite prepare error ({}): {} [SQL: {}]",
                rc,
                self.last_error(),
                sql
            );
        }
        Ok(SqliteStmt { db: self, stmt })
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe {
                sqlite3_close(self.db);
            }
            self.db = ptr::null_mut();
        }
    }
}

pub struct SqliteStmt<'a> {
    db: &'a SqliteDb,
    stmt: *mut sqlite3_stmt,
}

impl<'a> SqliteStmt<'a> {
    pub fn insert_row(&mut self, values: &[SqliteValue]) -> Result<()> {
        unsafe {
            for (i, val) in values.iter().enumerate() {
                let idx = (i + 1) as c_int;
                let rc = match val {
                    SqliteValue::Null => sqlite3_bind_null(self.stmt, idx),
                    SqliteValue::Integer(v) => sqlite3_bind_int64(self.stmt, idx, *v),
                    SqliteValue::Real(v) => sqlite3_bind_double(self.stmt, idx, *v),
                    SqliteValue::Text(s) => {
                        let ptr = if s.is_empty() {
                            b"\0".as_ptr() as *const c_char
                        } else {
                            s.as_ptr() as *const c_char
                        };
                        sqlite3_bind_text(
                            self.stmt,
                            idx,
                            ptr,
                            s.len() as c_int,
                            sqlite_transient(),
                        )
                    }
                    SqliteValue::Blob(b) => {
                        let ptr = if b.is_empty() {
                            b"\0".as_ptr() as *const c_void
                        } else {
                            b.as_ptr() as *const c_void
                        };
                        sqlite3_bind_blob(
                            self.stmt,
                            idx,
                            ptr,
                            b.len() as c_int,
                            sqlite_transient(),
                        )
                    }
                };
                if rc != SQLITE_OK {
                    bail!(
                        "SQLite bind error at column {}: {}",
                        idx,
                        self.db.last_error()
                    );
                }
            }

            let step_rc = sqlite3_step(self.stmt);
            sqlite3_reset(self.stmt);
            sqlite3_clear_bindings(self.stmt);

            if step_rc != SQLITE_DONE {
                bail!(
                    "SQLite step error ({}): {}",
                    step_rc,
                    self.db.last_error()
                );
            }
        }
        Ok(())
    }
}

impl<'a> Drop for SqliteStmt<'a> {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            unsafe {
                sqlite3_finalize(self.stmt);
            }
            self.stmt = ptr::null_mut();
        }
    }
}

pub fn quote_ident(name: &str) -> String {
    let clean: String = name.chars().filter(|&c| c != '\0').collect();
    format!("\"{}\"", clean.replace('"', "\"\""))
}

pub fn resolve_column_names(columns: &[ColumnDef]) -> std::collections::HashMap<i32, String> {
    let mut col_name_map = std::collections::HashMap::with_capacity(columns.len());
    let mut seen_names = std::collections::HashSet::new();
    for col in columns {
        let mut col_name = if col.name.is_empty() {
            format!("col_{}", col.colid)
        } else {
            col.name.clone()
        };
        let lower = col_name.to_lowercase();
        if !seen_names.insert(lower) {
            col_name = format!("{}_{}", col_name, col.colid);
            seen_names.insert(col_name.to_lowercase());
        }
        col_name_map.insert(col.colid, col_name);
    }
    col_name_map
}

pub fn build_create_table_sql(sqlite_table_name: &str, columns: &[ColumnDef]) -> String {
    let (sql, _) = build_create_table_sql_with_fks(
        sqlite_table_name,
        columns,
        &[],
        &std::collections::HashMap::new(),
    );
    sql
}

pub fn build_create_table_sql_with_fks(
    sqlite_table_name: &str,
    columns: &[ColumnDef],
    foreign_keys: &[ForeignKeyDef],
    all_tables_by_id: &std::collections::HashMap<i32, (String, std::collections::HashMap<i32, String>)>,
) -> (String, usize) {
    let col_name_map = resolve_column_names(columns);
    let mut parts = Vec::with_capacity(columns.len() + foreign_keys.len());

    for col in columns {
        if let Some(col_name) = col_name_map.get(&col.colid) {
            let decl_type = sql_type_to_sqlite_decl(col.xtype);
            parts.push(format!("{} {}", quote_ident(col_name), decl_type));
        }
    }

    let mut fk_count = 0;
    for fk in foreign_keys {
        let Some((ref_table_name, ref_col_map)) = all_tables_by_id.get(&fk.referenced_object_id)
        else {
            continue;
        };

        let mut parent_cols = Vec::with_capacity(fk.columns.len());
        let mut ref_cols = Vec::with_capacity(fk.columns.len());
        let mut valid = true;

        for fkc in &fk.columns {
            if let (Some(p_name), Some(r_name)) = (
                col_name_map.get(&fkc.parent_column_id),
                ref_col_map.get(&fkc.referenced_column_id),
            ) {
                parent_cols.push(quote_ident(p_name));
                ref_cols.push(quote_ident(r_name));
            } else {
                valid = false;
                break;
            }
        }

        if !valid || parent_cols.is_empty() {
            continue;
        }

        let mut fk_clause = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
            quote_ident(&fk.name),
            parent_cols.join(", "),
            quote_ident(ref_table_name),
            ref_cols.join(", ")
        );

        if let Some(del_sql) = fk.delete_action.to_sql_clause("DELETE") {
            fk_clause.push(' ');
            fk_clause.push_str(&del_sql);
        }
        if let Some(upd_sql) = fk.update_action.to_sql_clause("UPDATE") {
            fk_clause.push(' ');
            fk_clause.push_str(&upd_sql);
        }

        parts.push(fk_clause);
        fk_count += 1;
    }

    let sql = format!(
        "CREATE TABLE {} ({})",
        quote_ident(sqlite_table_name),
        parts.join(", ")
    );
    (sql, fk_count)
}

pub fn build_insert_sql(sqlite_table_name: &str, num_cols: usize) -> String {
    let placeholders: Vec<String> = (1..=num_cols).map(|i| format!("?{}", i)).collect();
    format!(
        "INSERT INTO {} VALUES ({});",
        quote_ident(sqlite_table_name),
        placeholders.join(", ")
    )
}

#[allow(dead_code)]
pub fn create_table_and_prepare<'a>(
    db: &'a SqliteDb,
    table: &UserTablePlan,
    has_duplicate_name: bool,
) -> Result<(String, SqliteStmt<'a>)> {
    let sqlite_name = table.sqlite_table_name(has_duplicate_name);
    let create_sql = build_create_table_sql(&sqlite_name, &table.columns);
    db.exec_batch(&create_sql)?;
    let insert_sql = build_insert_sql(&sqlite_name, table.columns.len());
    let stmt = db.prepare(&insert_sql)?;
    Ok((sqlite_name, stmt))
}

pub fn create_table_index(
    db: &SqliteDb,
    sqlite_table_name: &str,
    columns: &[ColumnDef],
    idx: &IndexDef,
    sqlite_index_name: &str,
) -> Result<bool> {
    // Resolve colid -> final SQLite column name
    let mut col_name_map = std::collections::HashMap::with_capacity(columns.len());
    let mut seen_names = std::collections::HashSet::new();
    for col in columns {
        let mut col_name = if col.name.is_empty() {
            format!("col_{}", col.colid)
        } else {
            col.name.clone()
        };
        let lower = col_name.to_lowercase();
        if !seen_names.insert(lower) {
            col_name = format!("{}_{}", col_name, col.colid);
            seen_names.insert(col_name.to_lowercase());
        }
        col_name_map.insert(col.colid, col_name);
    }

    let mut key_parts = Vec::with_capacity(idx.columns.len());
    for ic in &idx.columns {
        if let Some(cname) = col_name_map.get(&ic.colid) {
            let dir = if ic.is_descending { " DESC" } else { "" };
            key_parts.push(format!("{}{}", quote_ident(cname), dir));
        }
    }

    if key_parts.is_empty() {
        return Ok(false);
    }

    let cols_expr = key_parts.join(", ");
    if idx.is_unique {
        let unique_sql = format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS {} ON {} ({});",
            quote_ident(sqlite_index_name),
            quote_ident(sqlite_table_name),
            cols_expr
        );
        if db.exec_batch(&unique_sql).is_ok() {
            return Ok(true);
        }
    }

    let non_unique_sql = format!(
        "CREATE INDEX IF NOT EXISTS {} ON {} ({});",
        quote_ident(sqlite_index_name),
        quote_ident(sqlite_table_name),
        cols_expr
    );
    db.exec_batch(&non_unique_sql)?;
    Ok(true)
}

pub fn update_existing_table_ddl(
    db: &SqliteDb,
    sqlite_table_name: &str,
    new_create_sql: &str,
) -> Result<()> {
    let mut stmt =
        db.prepare("UPDATE sqlite_master SET sql = ?1 WHERE type = 'table' AND name = ?2;")?;
    stmt.insert_row(&[
        SqliteValue::Text(new_create_sql.to_string()),
        SqliteValue::Text(sqlite_table_name.to_string()),
    ])?;
    Ok(())
}
